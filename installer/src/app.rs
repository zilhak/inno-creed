//! 마법사 상태 머신 — 화면 전환과 각 화면의 렌더링.

use crate::{install, payload, platform};
#[cfg(target_os = "windows")]
use crate::registry;
use eframe::egui;
use std::path::PathBuf;
use std::time::{Duration, Instant};

enum Screen {
    Welcome,
    Confirm,
    AppRunning,
    Installing,
    ExtensionGuide,
    Done,
    Error(String),
    UninstallConfirm,
    UninstallAppRunning,
    UninstallDone,
}

pub struct InstallerApp {
    screen: Screen,
    config_path: Option<PathBuf>,
    config_confidence: Option<config_kit::ConfigCheck>,
    config_candidates: Vec<PathBuf>,
    install_dir: PathBuf,
    existing_version: Option<String>,
    new_version: Option<String>,
    install_result: Option<install::InstallResult>,
    doctor_output: Option<String>,
    doctor_ok: bool,
    doctor_expanded: bool,
    /// `platform::extension_browsers()`에서 고른 항목의 인덱스.
    ext_browser: usize,
    /// Claude Desktop 실행 여부를 마지막으로 확인한 시각(매 프레임 확인하면 과하다).
    last_app_check: Option<Instant>,
    /// 종료 요청을 보낸 시각. 이걸 기준으로 "그래도 안 꺼지면" 강제 종료를 내민다.
    quit_requested_at: Option<Instant>,
    /// 종료 시도가 왜 실패했는지. 삼키면 사용자는 버튼이 먹었는지조차 알 수 없다.
    quit_error: Option<String>,
    copied_at: Option<Instant>,
}

/// 후보 중 실제로 쓸 만한 것을 고른다. **존재만 하는 파일을 무조건 집지 않는다** —
/// MSIX 패키지 폴더가 낡아 남아있거나 내용이 깨진 파일을 그대로 골랐다가 설치
/// 단계에서야 파싱 실패로 터지면 원인을 알기 어렵다. `LooksLikeClaudeDesktop`을
/// 최우선으로 하고, 그게 하나도 없을 때만 `ParsedButUnfamiliar`로 물러선다.
fn pick_best_candidate(candidates: &[PathBuf]) -> Option<(PathBuf, config_kit::ConfigCheck)> {
    let mut fallback = None;
    for p in candidates {
        match config_kit::inspect_config(p) {
            config_kit::ConfigCheck::LooksLikeClaudeDesktop => {
                return Some((p.clone(), config_kit::ConfigCheck::LooksLikeClaudeDesktop));
            }
            check @ config_kit::ConfigCheck::ParsedButUnfamiliar if fallback.is_none() => {
                fallback = Some((p.clone(), check));
            }
            _ => {}
        }
    }
    fallback
}

impl Default for InstallerApp {
    fn default() -> Self {
        let candidates = config_kit::desktop_config_candidates();
        let (config_path, config_confidence) = match pick_best_candidate(&candidates) {
            Some((p, c)) => (Some(p), Some(c)),
            None => (None, None),
        };
        Self {
            screen: Screen::Welcome,
            config_path,
            config_confidence,
            config_candidates: candidates,
            install_dir: payload::default_install_dir(),
            existing_version: None,
            new_version: None,
            install_result: None,
            doctor_output: None,
            doctor_ok: false,
            doctor_expanded: false,
            ext_browser: 0,
            last_app_check: None,
            quit_requested_at: None,
            quit_error: None,
            copied_at: None,
        }
    }
}

impl InstallerApp {
    /// `--uninstall`로 실행됐을 때의 초기 상태. 설치 때와 같은 자동 감지 로직으로
    /// config 경로·설치 위치를 잡는다 — 별도 메타데이터 파일을 두지 않는다.
    pub fn new_uninstall() -> Self {
        Self {
            screen: Screen::UninstallConfirm,
            ..Self::default()
        }
    }
}

impl eframe::App for InstallerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(20.0);
            match &self.screen {
                Screen::Welcome => self.welcome_screen(ui),
                Screen::Confirm => self.confirm_screen(ui),
                Screen::AppRunning => self.app_running_screen(ui),
                Screen::Installing => self.installing_screen(ui),
                Screen::ExtensionGuide => self.extension_guide_screen(ui),
                Screen::Done => self.done_screen(ui),
                Screen::Error(_) => self.error_screen(ui),
                Screen::UninstallConfirm => self.uninstall_confirm_screen(ui),
                Screen::UninstallAppRunning => self.uninstall_app_running_screen(ui),
                Screen::UninstallDone => self.uninstall_done_screen(ui),
            }
        });
    }
}

impl InstallerApp {
    fn welcome_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.heading("inno-creed 설치");
            ui.add_space(12.0);
            ui.label("이노그리드 아마란스를 Claude로 다루는 inno-creed를 설치합니다.");
            ui.label("Claude Desktop 앱의 채팅·Cowork 탭에서 곧바로 쓸 수 있게 등록해 드립니다.");
            ui.add_space(20.0);

            ui.group(|ui| {
                ui.set_width(460.0);
                ui.label("⚠️  아직 Claude Desktop이 없다면, 먼저 설치한 뒤 이 프로그램을 다시 실행하세요.");
                if ui.link("claude.ai/download 열기").clicked() {
                    let _ = open::that("https://claude.ai/download");
                }
            });
            // 이 안내는 Windows에서만 뜻이 있다. macOS의 차단은 문구가 다르고 절차도 달라서
            // (시스템 설정 → 개인정보 보호 및 보안 → 그래도 열기) 여기서 안내할 수 없고,
            // 애초에 그 화면을 넘긴 사람만 이 창을 보고 있다.
            #[cfg(target_os = "windows")]
            {
                ui.add_space(12.0);
                ui.group(|ui| {
                    ui.set_width(460.0);
                    ui.label(
                        "Windows가 \"PC를 보호했습니다\" 경고를 띄우면 [추가 정보] → [실행]을 눌러주세요.",
                    );
                });
            }

            ui.add_space(28.0);
            if ui
                .add(egui::Button::new("다음 →").min_size(egui::vec2(140.0, 36.0)))
                .clicked()
            {
                if let Err(e) = payload::verify_payload_present() {
                    self.screen = Screen::Error(e);
                } else {
                    let dest_bin = self.install_dir.join(payload::inno_creed_binary_name());
                    self.existing_version = install::read_version(&dest_bin);
                    self.new_version = install::read_version(&payload::payload_binary_path());
                    self.screen = Screen::Confirm;
                }
            }
        });
    }

    fn confirm_screen(&mut self, ui: &mut egui::Ui) {
        ui.heading("설치 위치 확인");
        ui.add_space(16.0);

        ui.label("Claude Desktop 설정 파일:");
        match &self.config_path {
            Some(p) => {
                ui.monospace(p.display().to_string());
                match &self.config_confidence {
                    Some(config_kit::ConfigCheck::LooksLikeClaudeDesktop) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(40, 140, 60),
                            "✅ 파일을 직접 열어 확인했습니다 — Claude Desktop 설정이 맞습니다.",
                        );
                    }
                    Some(config_kit::ConfigCheck::ParsedButUnfamiliar) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 140, 30),
                            "⚠️ 파일은 읽히지만 낯선 내용입니다 — 다른 프로그램이 만든 파일일 수도 있습니다. 확실하지 않으면 [직접 선택...]으로 다시 골라주세요.",
                        );
                    }
                    Some(config_kit::ConfigCheck::ParseFailed) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 90, 60),
                            "❌ 파일이 있지만 JSON으로 읽히지 않습니다 — 이대로 설치하면 실패합니다. [직접 선택...]으로 다른 파일을 고르거나, 파일을 열어 문법 오류를 먼저 고쳐주세요.",
                        );
                    }
                    _ => {}
                }
            }
            None => {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 90, 60),
                    "찾지 못했습니다. Claude Desktop이 설치되어 있지 않거나, 설치는 됐지만 \
                     한 번도 실행한 적이 없을 수 있습니다(설정 파일은 처음 실행할 때 만들어집니다).",
                );
                if ui.link("claude.ai/download 열기").clicked() {
                    let _ = open::that("https://claude.ai/download");
                }
                ui.add_space(4.0);
                for c in &self.config_candidates {
                    ui.small(format!("  (확인한 경로) {}", c.display()));
                }
            }
        }
        if ui.button("직접 선택...").clicked() {
            if let Some(picked) = rfd::FileDialog::new()
                .add_filter("Claude 설정 파일", &["json"])
                .set_file_name("claude_desktop_config.json")
                .pick_file()
            {
                self.config_confidence = Some(config_kit::inspect_config(&picked));
                self.config_path = Some(picked);
            }
        }

        ui.add_space(16.0);
        ui.label("inno-creed를 놓을 위치:");
        ui.monospace(self.install_dir.display().to_string());
        match (&self.existing_version, &self.new_version) {
            (Some(old), Some(new)) if old == new => {
                ui.label(format!("이미 v{old}이 설치돼 있습니다 — 같은 버전을 다시 설치합니다."));
            }
            (Some(old), Some(new)) => {
                ui.colored_label(egui::Color32::from_rgb(40, 140, 60), format!("업데이트: v{old} → v{new}"));
            }
            (None, Some(new)) => {
                ui.small(format!("새로 설치합니다 (v{new}).")); // 이 위치에 처음 설치
            }
            (Some(old), None) => {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 90, 60),
                    format!("⚠️ 기존 v{old}이 있지만 새 버전 확인에 실패했습니다."),
                );
            }
            (None, None) => {}
        }
        if ui.button("다른 폴더 선택...").clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                self.install_dir = dir.join("inno-creed");
                let dest_bin = self.install_dir.join(payload::inno_creed_binary_name());
                self.existing_version = install::read_version(&dest_bin);
            }
        }

        ui.add_space(24.0);
        ui.horizontal(|ui| {
            if ui.button("← 이전").clicked() {
                self.screen = Screen::Welcome;
            }
            let can_install = self.config_path.is_some();
            if ui
                .add_enabled(can_install, egui::Button::new("설치").min_size(egui::vec2(120.0, 32.0)))
                .clicked()
            {
                self.screen = if config_kit::is_claude_desktop_running() {
                    Screen::AppRunning
                } else {
                    Screen::Installing
                };
            }
        });
    }

    /// 꺼졌는지 **스스로** 1초마다 확인한다. 사용자가 직접 껐든 아래 버튼으로 껐든,
    /// "다시 확인"을 누르게 만들지 않기 위해서다. 매 프레임 확인하지 않는 것은
    /// `is_claude_desktop_running`이 프로세스 목록을 통째로 훑기 때문이다.
    fn claude_still_running(&mut self, ctx: &egui::Context) -> bool {
        ctx.request_repaint_after(Duration::from_millis(300));
        let now = Instant::now();
        let due = self
            .last_app_check
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1));
        if !due {
            return true;
        }
        self.last_app_check = Some(now);
        config_kit::is_claude_desktop_running()
    }

    fn app_running_screen(&mut self, ui: &mut egui::Ui) {
        if !self.claude_still_running(ui.ctx()) {
            self.quit_requested_at = None;
            self.quit_error = None;
            self.screen = Screen::Installing;
            return;
        }
        ui.vertical_centered(|ui| {
            ui.heading("Claude Desktop을 종료해주세요");
            ui.add_space(12.0);
            ui.label("설정 파일을 안전하게 쓰려면 Claude Desktop이 완전히 꺼져 있어야 합니다.");
            ui.label("(창을 닫아도 메뉴 막대·트레이에 남아있을 수 있습니다 — 아래 버튼으로 대신 꺼드립니다.)");

            // 무엇을 보고 그렇게 판단했는지 밝힌다. "다 껐는데 왜?"에서 막히지 않게 하는 것이 핵심이다.
            let found = config_kit::running_claude_desktop_processes();
            if !found.is_empty() {
                ui.add_space(10.0);
                ui.group(|ui| {
                    ui.set_width(460.0);
                    ui.small("지금 실행 중인 것으로 확인된 프로세스:");
                    for (pid, exe) in found.iter().take(4) {
                        ui.small(format!("  pid {pid}  {exe}"));
                    }
                    if found.len() > 4 {
                        ui.small(format!("  … 외 {}개", found.len() - 4));
                    }
                });
            }
            ui.add_space(20.0);
            if ui
                .add(egui::Button::new("Claude Desktop 종료하기").min_size(egui::vec2(200.0, 34.0)))
                .clicked()
            {
                self.quit_error = platform::request_quit_claude_desktop().err();
                self.quit_requested_at = Some(Instant::now());
                // 방금 보낸 요청이 반영될 틈을 준다 — 곧바로 다시 세면 아직 살아 있다.
                self.last_app_check = Some(Instant::now());
            }
            ui.small("종료 요청을 보냅니다. 꺼지면 설치가 저절로 이어집니다.");
            if let Some(err) = &self.quit_error {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(200, 90, 60), format!("⚠️ {err}"));
            }

            // 종료 요청을 보냈는데도 안 꺼지는 경우가 있다(트레이에만 남는 구현 등).
            // 그때만 강제 종료를 내민다 — 처음부터 보여주면 눌러도 되는 버튼처럼 보인다.
            if self
                .quit_requested_at
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(6))
            {
                ui.add_space(14.0);
                ui.group(|ui| {
                    ui.set_width(420.0);
                    ui.label("아직 꺼지지 않았습니다. 강제로 종료할 수 있습니다.");
                    ui.small("저장하지 않은 대화나 작업이 있으면 잃을 수 있습니다.");
                    if ui.button("강제 종료").clicked() {
                        self.quit_error = platform::force_quit_claude_desktop().err();
                        self.last_app_check = Some(Instant::now());
                    }
                });
            }

            ui.add_space(20.0);
            if ui.button("← 이전").clicked() {
                self.quit_requested_at = None;
                self.quit_error = None;
                self.screen = Screen::Confirm;
            }
        });
    }

    fn installing_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.heading("설치 중...");
        });
        let Some(config_path) = self.config_path.clone() else {
            self.screen = Screen::Error("설정 파일 경로가 지정되지 않았습니다.".into());
            return;
        };
        let src_bin = payload::payload_binary_path();
        let src_ext = payload::payload_extension_dir();
        let src_ext = src_ext.exists().then_some(src_ext.as_path());
        match install::perform_install(
            &config_path,
            &self.install_dir,
            &src_bin,
            src_ext,
            payload::inno_creed_binary_name(),
        ) {
            Ok(result) => {
                // "프로그램 추가/제거" 등록은 있으면 좋은 부가 기능이라, 실패해도
                // 설치 자체를 막지 않는다(레지스트리 접근이 막힌 사내 정책 등 대비).
                #[cfg(target_os = "windows")]
                {
                    if let Ok(installer_copy) =
                        install::copy_installer_self(&self.install_dir, "installer.exe")
                    {
                        let _ = registry::register_uninstall_entry(&self.install_dir, &installer_copy);
                    }
                }
                self.screen = if result.extension_dir.is_some() {
                    Screen::ExtensionGuide
                } else {
                    Screen::Done
                };
                self.install_result = Some(result);
            }
            Err(e) => self.screen = Screen::Error(format!("설치 중 오류가 발생했습니다: {e:#}")),
        }
    }

    /// 복사 직후 잠깐 뜨는 알림. 사용자 입력 없이도 스스로 사라져야 하므로
    /// 떠 있는 동안은 다시 그려달라고 요청한다.
    fn copy_toast(&self, ctx: &egui::Context) {
        const TOAST: Duration = Duration::from_millis(2200);
        let Some(at) = self.copied_at else { return };
        if at.elapsed() >= TOAST {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        egui::Area::new(egui::Id::new("copy_toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgb(40, 120, 60))
                    .show(ui, |ui| {
                        ui.colored_label(
                            egui::Color32::WHITE,
                            "✅ 주소가 복사되었습니다. 주소창에 붙여넣으세요.",
                        );
                    });
            });
    }

    fn extension_guide_screen(&mut self, ui: &mut egui::Ui) {
        ui.heading("확장 프로그램 연결");
        ui.add_space(12.0);
        let browsers = platform::extension_browsers();
        let has_edge = browsers.iter().any(|b| b.name == "Edge");
        let names = browsers.iter().map(|b| b.name).collect::<Vec<_>>().join("/");
        ui.label(format!(
            "아마란스 로그인 정보를 안전하게 가져오려면 {names} 확장 프로그램을 마저 등록해야 합니다. \
             아직 안 하면 로그인 인증이 안 잡힙니다."
        ));
        ui.add_space(8.0);
        ui.label("1. 아래 [확장 폴더 열기]로 열리는 폴더를 기억해두세요.");
        ui.label("2. 아래 주소를 복사해 브라우저 주소창에 붙여넣어 확장 관리 화면을 열고 개발자 모드를 켭니다.");
        if has_edge {
            ui.label("   (Chrome은 화면 우측 상단, Edge는 화면 좌측 하단에 토글이 있습니다)");
            ui.label("3. \"압축해제된 확장 프로그램을 로드합니다\"(Edge는 \"압축 풀린 파일 로드\")를 눌러 방금 그 폴더를 선택합니다.");
        } else {
            ui.label("   (토글은 화면 우측 상단에 있습니다)");
            ui.label("3. \"압축해제된 확장 프로그램을 로드합니다\"를 눌러 방금 그 폴더를 선택합니다.");
        }
        ui.label("4. 목록에 \"inno-creed 크레덴셜 브릿지\" 카드가 뜨고 토글이 켜져 있으면 성공입니다.");
        ui.add_space(16.0);

        // Ctrl+C · Cmd+C · Ctrl+Shift+C는 egui-winit에서 전부 `Event::Copy` 하나로 들어온다.
        // 이벤트를 여기서 걷어내는 것은, 그대로 두면 아래 주소 라벨의 **부분 선택** 복사가
        // 패스 끝에 우리 복사를 덮어써서 "무조건 주소 전체"가 깨지기 때문이다.
        let copy_shortcut = ui.input_mut(|i| {
            let hit = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            i.events.retain(|e| !matches!(e, egui::Event::Copy));
            hit
        });

        ui.horizontal(|ui| {
            if let Some(ext_dir) = self.install_result.as_ref().and_then(|r| r.extension_dir.clone()) {
                if ui.button("📁 확장 폴더 열기").clicked() {
                    let _ = open::that(ext_dir);
                }
            }
            if browsers.len() > 1 {
                for (i, b) in browsers.iter().enumerate() {
                    ui.selectable_value(&mut self.ext_browser, i, format!("{} 주소", b.name));
                }
            }
        });

        ui.add_space(8.0);
        let url = browsers[self.ext_browser.min(browsers.len() - 1)].url;
        let mut copy_clicked = false;
        ui.horizontal(|ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(url).monospace().size(15.0)).selectable(true));
            });
            copy_clicked = ui.button("📋 복사").clicked();
        });
        ui.small("드래그해서 복사하거나, [복사] 버튼 또는 Ctrl+C / Cmd+C / Ctrl+Shift+C를 누르세요.");

        if copy_clicked || copy_shortcut {
            ui.ctx().copy_text(url.to_owned());
            self.copied_at = Some(Instant::now());
        }
        self.copy_toast(ui.ctx());

        if has_edge {
            ui.add_space(16.0);
            ui.group(|ui| {
                ui.set_width(460.0);
                ui.label(
                    "⚠️  Edge를 새로 시작하면 \"개발자 모드에서 확장 사용 해제\" 경고 팝업이 뜰 수 \
                     있습니다. 여기서 [확장 사용 해제]를 누르면 방금 설치한 확장이 꺼집니다 — \
                     이 버튼은 누르지 말고 [나중에]를 누르세요.",
                );
            });
        }

        ui.add_space(24.0);
        if ui
            .add(egui::Button::new("다음 →").min_size(egui::vec2(140.0, 36.0)))
            .clicked()
        {
            self.screen = Screen::Done;
        }
    }

    fn done_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.heading("설치 완료");
            ui.add_space(12.0);

            if self.doctor_output.is_none() {
                if let Some(result) = &self.install_result {
                    match install::run_doctor(&result.exe_path) {
                        Ok(out) => {
                            self.doctor_ok = out.contains("✅ 인증 성공");
                            self.doctor_output = Some(out);
                        }
                        Err(e) => {
                            self.doctor_output = Some(format!("doctor 실행 실패: {e:#}"));
                        }
                    }
                }
            }

            if self.doctor_ok {
                ui.colored_label(egui::Color32::from_rgb(40, 140, 60), "✅ 인증까지 확인됐습니다. 바로 쓸 수 있습니다.");
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 140, 30),
                    "⚠️ 등록은 됐지만 인증 확인은 안 됐습니다 — 자세히 보기에서 원인을 확인하세요.",
                );
            }
            ui.label("Claude Desktop을 (다시) 실행하면 채팅·Cowork 탭에서 inno-creed 도구를 쓸 수 있습니다.");
            ui.small("Code 탭·Claude Code CLI는 설정 파일이 따로입니다 — 거기서도 쓰려면 `claude mcp add`로 한 번 더 등록하세요.");

            if let Some(bak) = self.install_result.as_ref().and_then(|r| r.backup_path.clone()) {
                ui.add_space(6.0);
                ui.small(format!("기존 설정은 백업해뒀습니다: {}", bak.display()));
            }

            ui.add_space(12.0);
            ui.checkbox(&mut self.doctor_expanded, "자세히 보기 (doctor 전체 출력)");
            if self.doctor_expanded {
                if let Some(out) = &self.doctor_output {
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        ui.monospace(out);
                    });
                }
            }
        });
    }

    fn error_screen(&mut self, ui: &mut egui::Ui) {
        let message = if let Screen::Error(m) = &self.screen {
            m.clone()
        } else {
            String::new()
        };
        ui.vertical_centered(|ui| {
            ui.heading("문제가 발생했습니다");
            ui.add_space(12.0);
            ui.colored_label(egui::Color32::from_rgb(200, 60, 50), &message);
            ui.add_space(20.0);
            if ui.button("← 처음으로").clicked() {
                self.screen = Screen::Welcome;
            }
        });
    }

    fn uninstall_confirm_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.heading("inno-creed 제거");
            ui.add_space(12.0);
            ui.label("Claude Desktop 설정에서 inno-creed 등록을 지우고, 설치된 파일을 삭제합니다.");
            ui.add_space(8.0);
            match &self.config_path {
                Some(p) => {
                    ui.small(format!("설정 파일: {}", p.display()));
                }
                None => {
                    ui.colored_label(egui::Color32::from_rgb(200, 90, 60), "설정 파일을 찾지 못했습니다 — 등록 해제는 건너뜁니다.");
                }
            }
            ui.small(format!("삭제할 폴더: {}", self.install_dir.display()));
            ui.add_space(20.0);
            if ui
                .add(egui::Button::new("제거").min_size(egui::vec2(120.0, 32.0)))
                .clicked()
            {
                if config_kit::is_claude_desktop_running() {
                    self.screen = Screen::UninstallAppRunning;
                } else {
                    self.do_uninstall();
                }
            }
        });
    }

    fn uninstall_app_running_screen(&mut self, ui: &mut egui::Ui) {
        if !self.claude_still_running(ui.ctx()) {
            self.quit_requested_at = None;
            self.quit_error = None;
            self.do_uninstall();
            return;
        }
        ui.vertical_centered(|ui| {
            ui.heading("Claude Desktop을 종료해주세요");
            ui.add_space(12.0);
            ui.label("설정 파일을 안전하게 고치려면 Claude Desktop이 완전히 꺼져 있어야 합니다.");
            ui.label("(창을 닫아도 메뉴 막대·트레이에 남아있을 수 있습니다 — 아래 버튼으로 대신 꺼드립니다.)");
            let found = config_kit::running_claude_desktop_processes();
            if !found.is_empty() {
                ui.add_space(10.0);
                ui.group(|ui| {
                    ui.set_width(460.0);
                    ui.small("지금 실행 중인 것으로 확인된 프로세스:");
                    for (pid, exe) in found.iter().take(4) {
                        ui.small(format!("  pid {pid}  {exe}"));
                    }
                });
            }
            ui.add_space(20.0);
            if ui
                .add(egui::Button::new("Claude Desktop 종료하기").min_size(egui::vec2(200.0, 34.0)))
                .clicked()
            {
                self.quit_error = platform::request_quit_claude_desktop().err();
                self.quit_requested_at = Some(Instant::now());
                self.last_app_check = Some(Instant::now());
            }
            if let Some(err) = &self.quit_error {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(200, 90, 60), format!("⚠️ {err}"));
            }
            if self
                .quit_requested_at
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(6))
            {
                ui.add_space(14.0);
                if ui.button("강제 종료").clicked() {
                    self.quit_error = platform::force_quit_claude_desktop().err();
                    self.last_app_check = Some(Instant::now());
                }
            }
        });
    }

    /// **화면 전환까지 여기서 끝낸다.** 예전에는 호출부가
    /// `self.do_uninstall(); Screen::UninstallDone` 꼴로 화면을 덮어써서, 여기서
    /// `Screen::Error`를 세워도 곧바로 "제거 완료"로 지워졌다 — 실패가 성공으로
    /// 보고됐다. 그래서 반환값 대신 `self.screen`을 직접 세운다.
    fn do_uninstall(&mut self) {
        let installer_copy = self.install_dir.join("installer.exe");
        // config를 못 찾았어도 파일 삭제는 진행한다(확인 화면에서 "등록 해제는
        // 건너뜁니다"라고 이미 예고한 동작이다).
        if let Err(e) = install::perform_uninstall(
            self.config_path.as_deref(),
            &self.install_dir,
            &installer_copy,
        ) {
            self.screen = Screen::Error(format!("제거 중 오류가 발생했습니다: {e:#}"));
            return;
        }
        #[cfg(target_os = "windows")]
        registry::remove_uninstall_entry();
        install::schedule_self_delete(&installer_copy, &self.install_dir);
        self.screen = Screen::UninstallDone;
    }

    fn uninstall_done_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.heading("제거 완료");
            ui.add_space(12.0);
            // 등록 해제를 실제로 했을 때만 그렇게 말한다 — config를 못 찾은 채로
            // "등록을 지웠습니다"라고 하면 남아있는 등록을 없는 것으로 착각한다.
            if self.config_path.is_some() {
                ui.label("inno-creed 등록을 지우고 설치된 파일을 삭제했습니다. Claude Desktop을 다시 실행하면 반영됩니다.");
            } else {
                ui.label("설치된 파일을 삭제했습니다.");
                ui.add_space(8.0);
                ui.colored_label(
                    egui::Color32::from_rgb(200, 90, 60),
                    "Claude Desktop 설정 파일을 찾지 못해 등록은 그대로 남아 있습니다 —\n\
                     설정에서 inno-creed 항목을 직접 지워주세요.",
                );
            }
            ui.add_space(8.0);
            ui.small("이 창은 닫아도 됩니다.");
        });
    }
}
