# 문서 색인

inno-creed의 기술 문서. 프로젝트 소개·도구 목록은 [루트 README](../README.md)를 보세요.

| 문서 | 내용 |
|---|---|
| [INSTALL.md](INSTALL.md) | 설치 가이드 — GUI 인스톨러(비개발자 권장) · OS별 수동 설치 · MCP 등록 · Chrome/Edge 확장 프로그램 · 크레덴셜 문제 해결 |
| [index.html](index.html) | 위 설치 가이드의 웹 랜딩페이지 — Claude 환경(CLI/Code/Cowork/웹)을 먼저 고르게 하는 탭 구성 |
| [installer-guide.html](installer-guide.html) | 설치 프로그램으로 설치하기 — 내려받기부터 확장 프로그램 연결까지 실제 화면 캡처 8단계 ([웹](https://zilhak.github.io/inno-creed/installer-guide.html)) |
| [installer-cli.html](installer-cli.html) | 창이 안 뜰 때 쓰는 `installer-cli` 안내 — Enter만 눌러도 설치되는 흐름과 각 물음의 뜻 ([웹](https://zilhak.github.io/inno-creed/installer-cli.html)) |
| [mac-first-run.html](mac-first-run.html) | macOS에서 인스톨러가 "열 수 없음"으로 차단될 때 허용하는 방법 — 차단 화면부터 시스템 설정 [그래도 열기]까지 캡처 6단계 ([웹](https://zilhak.github.io/inno-creed/mac-first-run.html)) |
| [extension-install.html](extension-install.html) | Chrome/Edge 확장 프로그램 설치 방법 — 다운로드부터 아마란스 로그인까지 실제 화면 캡처 12단계 ([웹](https://zilhak.github.io/inno-creed/extension-install.html)) |
| [architecture.md](architecture.md) | 브라우저가 필요 없는 이유, 모듈 구조, 크레덴셜 취득(확장 브릿지 · 브라우저 쿠키 복호화), authToken·세션 정보, `wehago-sign` 서명 규격, 응답 봉투, 안전 규약(read-back·소유권 가드) |
| [api-reference.md](api-reference.md) | 모듈별 확정 API 스키마 — 자원(회의실)·일정·메일·게시판·전자결재·근태·조직도의 엔드포인트, 요청/응답 필드, 실측으로 확인한 함정 |
| [HTTP.md](HTTP.md) | HTTP 전송을 정식 지원하지 않는 이유(크레덴셜=서버 머신 소유자, 되돌릴 수 없는 쓰기 도구, 서버 기준 파일 경로)와, 로컬 한정으로 꼭 필요할 때 직접 빌드하는 절차 |
| [../tests/live/README.md](../tests/live/README.md) | 라이브 스모크 테스트 — 실제 아마란스에 붙어 도구 59개의 호출·제외 여부를 검사하는 하네스의 안전장치(CI 차단·마커 기반 삭제 가드·잔여물 대장)와 실행법 |

## 이 문서들의 원칙

- **실증으로 확정된 사실만 담는다.** 추측·미확인 내용은 넣지 않는다. "될 것 같다"는 문서화하지 않고, 실제 호출해서 응답을 확인한 것만 적는다.
- **조사 과정·미확정 가설·캡처 원본은 여기 넣지 않는다.** 결론만 남기고, 그 결론이 무엇으로 확인됐는지를 함께 적는다.
- 값이 사람마다 다른 것(사번·테넌트 ID 등)은 자리표시자로 쓴다. 실제 값을 문서에 박지 않는다.
