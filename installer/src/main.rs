// inno-creed 초보자용 GUI 인스톨러 — inno-creed 본체와 완전히 분리된 별개 산출물.
// 배포 zip에 installer와 나란히 놓인 payload/를 찾아 설치하고, config-kit으로
// claude_desktop_config.json에 등록한다.

// 기본 "콘솔" 서브시스템으로 빌드되면 GUI 창과 별개로 검은 콘솔창이 뒤에 함께 뜬다
// (Windows 전용 속성 — 다른 OS는 원래 이런 구분이 없어 그냥 무시된다).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod fatal;
mod install;
mod payload;
#[cfg(target_os = "windows")]
mod registry;

fn main() {
    // 무엇보다 먼저. 이 아래에서 벌어지는 어떤 실패도 창으로 보이게 하는 장치다.
    fatal::install_panic_hook();

    if let Err(err) = run() {
        fatal::report(&format!(
            "설치 프로그램 창을 띄우지 못했습니다.\n\n\
             내용: {err}\n\
             (원문: {err:?})\n\n\
             그래픽 초기화 실패라면 아래를 PowerShell에서 실행해 우회할 수 있습니다.\n\
             \x20 $env:WGPU_BACKEND=\"dx12\"; .\\installer.exe"
        ));
        std::process::exit(1);
    }
}

fn run() -> eframe::Result<()> {
    let uninstall = std::env::args().any(|a| a == "--uninstall");
    let title = if uninstall { "inno-creed 제거" } else { "inno-creed 설치" };
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("bundled icon.png must decode");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 480.0])
            .with_resizable(false)
            .with_icon(icon),
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(move |cc| {
            setup_korean_font(&cc.egui_ctx);
            let app = if uninstall {
                app::InstallerApp::new_uninstall()
            } else {
                app::InstallerApp::default()
            };
            Ok(Box::new(app))
        }),
    )
}

/// eframe 기본값은 `power_preference: HighPerformance` + `force_fallback_adapter: false`라,
/// 하이브리드 그래픽 노트북에서 외장 GPU를 요구하다 실패하면 그대로 초기화가 깨진다.
/// 이 창은 버튼과 글자만 그리므로 GPU 성능이 전혀 필요 없다 — 내장 GPU를 먼저 고르고,
/// 그것마저 없으면 소프트웨어 렌더러(WARP)까지 받아들여 "일단 뜨는" 쪽을 택한다.
///
/// `force_fallback_adapter`는 egui-wgpu가 노출하지 않아(`egui-wgpu/src/lib.rs`의 주석이
/// 이 방법을 안내한다) 어댑터 선택기를 직접 끼우는 것이 유일한 경로다. 선택기를 설정하면
/// `power_preference` 필드는 무시되므로, 선호 순서는 아래 `adapter_rank`가 전부 담는다.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut options.wgpu_setup {
        create_new.native_adapter_selector = Some(std::sync::Arc::new(select_adapter));
    }
    options
}

fn select_adapter(
    adapters: &[eframe::wgpu::Adapter],
    surface: Option<&eframe::wgpu::Surface<'_>>,
) -> Result<eframe::wgpu::Adapter, String> {
    use eframe::wgpu;

    // 진단용으로 남겨둔 탈출구. WGPU_POWER_PREF=high면 기존 동작(외장 GPU 우선)으로 돌아간다.
    let prefer_high = wgpu::PowerPreference::from_env() == Some(wgpu::PowerPreference::HighPerformance);

    adapters
        .iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .min_by_key(|a| adapter_rank(a.get_info().device_type, prefer_high))
        .cloned()
        .ok_or_else(|| describe_adapters(adapters, surface))
}

fn adapter_rank(device_type: eframe::wgpu::DeviceType, prefer_high: bool) -> u8 {
    use eframe::wgpu::DeviceType;
    match device_type {
        DeviceType::IntegratedGpu => u8::from(prefer_high),
        DeviceType::DiscreteGpu => u8::from(!prefer_high),
        DeviceType::VirtualGpu => 2,
        DeviceType::Other => 3,
        // 마지막 보루. 느리지만 이 화면을 그리기엔 충분하고, 아무것도 안 뜨는 것보다 낫다.
        DeviceType::Cpu => 4,
    }
}

/// 실패했을 때 이 문자열이 그대로 오류 창에 실린다. "왜 하나도 못 골랐는지"를
/// 사용자 화면에서 바로 읽을 수 있어야 원격으로 판별이 된다.
fn describe_adapters(
    adapters: &[eframe::wgpu::Adapter],
    surface: Option<&eframe::wgpu::Surface<'_>>,
) -> String {
    if adapters.is_empty() {
        return "이 PC에서 사용할 수 있는 그래픽 어댑터를 하나도 찾지 못했습니다 \
                (Vulkan/DX12/OpenGL 모두 실패). 그래픽 드라이버를 설치하거나 업데이트해 주세요."
            .to_owned();
    }
    let list = adapters
        .iter()
        .map(|a| {
            let info = a.get_info();
            let compat = match surface {
                Some(s) if !a.is_surface_supported(s) => " [이 창과 호환되지 않음]",
                _ => "",
            };
            format!(
                "  - {} / {:?} / {:?} / 드라이버 {}{compat}",
                info.name, info.device_type, info.backend, info.driver
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("찾은 그래픽 어댑터 {}개가 모두 이 창과 호환되지 않습니다:\n{list}", adapters.len())
}

/// egui 기본 폰트에는 한글 글리프가 없어 아무 설정 없이 그리면 네모(tofu)만 뜬다.
/// 각 OS에 기본으로 깔린 한글 폰트를 읽어 등록한다.
fn setup_korean_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        "C:/Windows/Fonts/malgun.ttf",
        "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
        "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
    ];
    if let Some(path) = candidates.iter().find(|p| std::path::Path::new(p).exists()) {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "korean".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "korean".to_owned());
            // Monospace에도 넣는다 — doctor 전체 출력을 `ui.monospace`로 그리는데
            // 여기에 없으면 그 안의 한글만 네모로 깨진다. 이쪽은 맨 뒤에 붙여
            // ASCII는 고정폭 그대로 두고 한글만 넘어오게 한다.
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("korean".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}
