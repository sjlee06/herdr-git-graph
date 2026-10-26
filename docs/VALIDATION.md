# 검증 기록

2026-09-12, macOS Apple Silicon에서 검증했습니다.

## 환경

- Rust 1.98.1 / Cargo stable
- Ratatui 0.30.2 / Crossterm 0.29.0 / tiny-skia 0.11.4
- Git 2.54.0
- Herdr 0.9.0의 설치된 바이너리 및 `herdr api schema` 출력
- Python 표준 라이브러리 기반 PTY·Unix socket 테스트

## 통과한 항목

- `cargo fmt --check`
- `cargo clippy --all-targets --locked -- -D warnings`
- `cargo test --locked`: 단위 테스트 12개 + Herdr 실행 인자 통합 테스트 4개 + 실제 Git 저장소 통합 테스트 4개
- `scripts/build.sh`: 최적화한 macOS arm64 실행 파일 생성
- 병합 그래프의 각 outgoing edge가 실제 부모 OID와 일치하는지 검증
- 여러 부모 병합, 연결되지 않은 이력, 페이지 밖 부모, 빈 저장소, 빈 커밋 제목
- 한글 메시지와 검색, detached HEAD, linked worktree, shallow clone, 브랜치 필터
- 실제 diff 내용, 조회 전후 working tree 상태 일치, external diff 비활성화
- 여러 터미널 크기 및 도움말 화면을 Ratatui TestBackend로 렌더링
- SIGTERM 종료 시 raw mode·alternate screen·마우스 캡처 복구
- 가짜 Herdr 서버와 실제 실행 파일 간 그래픽 협상, PNG 프레임 전송, 크기 변경
- 도움말 표시 및 종료 시 그래픽 스트림/레이어 수명 종료
- 그래픽 API가 `feature_disabled`를 반환할 때 문자 모드 전환
- Herdr 0.9.0 CLI로 매니페스트를 읽고 `plugin_linked` 응답 확인
- 같은 Ratatui 화면 버퍼에서 만든 SVG와 tiny-skia PNG를 시각적으로 검토

초기 매니페스트 검증은 별도 작업 폴더의 XDG 설정·상태 디렉터리와 비활성 플러그인 등록으로 수행했습니다.

## 사이드바 및 단축키 수정 검증

- 설치된 Herdr 0.9.0의 `--default-config`로 `prefix+g`의 `goto`, `prefix+shift+g`의 워크트리 생성 충돌 확인. README의 `prefix+u` / `prefix+shift+u` 예제를 임시 설정 파일에서 `herdr config check`로 검증.
- `--open-sidebar`의 오른쪽 분할·포커스 유지, `--open-pane`의 기존 탭 열기, 공백이 있는 저장소 경로 및 작업 공간·원본 패널 전달, 실행 실패와 CLI 옵션 충돌 검증.
- 사이드바 24×8, 32×12, 48×30, 140×44 화면에서 그래프 표시·선택 행 가시성·숨긴 패널의 마우스 영역 제거 확인. diff 요청 생략과 검색·새로고침 유지 확인.
- PTY에서 좁은 사이드바의 한글 검색·키보드·마우스·크기 변경·종료 복구, 모의 Herdr 서버에서 곡선 전송·수평 이동·도움말·종료 시 레이어 정리 검증.
- 40×24 곡선 사이드바, 24×8 문자 사이드바, 기존 전체 화면의 SVG를 PNG로 렌더링해 시각 검토. 로컬 릴리스 빌드를 플러그인으로 연결하고 새 `sidebar` 액션 등록 확인.

## v0.1.1 설치 수정 검증

- [릴리스 워크플로](https://github.com/sjlee06/herdr-git-graph/actions/runs/34683222669): macOS·Linux 각각 arm64/x86_64의 네 환경에서 빌드, Rust 테스트, PTY·그래픽 모의 서버 테스트 통과.
- `tests/install.py`: 운영체제별 파일 선택, 체크섬 검증, 다운로드 실패, 기존 파일 보존 등 설치 테스트 6개 통과.
- Cargo가 없는 `PATH`를 사용해 실제 공개 GitHub 저장소에 `herdr plugin install sjlee06/herdr-git-graph --yes`를 실행하고, 이전 v0.1.0 설치가 v0.1.1로 교체되는 것 확인.
- 설치 검증은 별도 XDG 설정·상태 폴더에서 수행했으며, 사용자 설정은 변경하지 않았습니다.

## v0.2.1 사이드바 실행 수정 검증

- 사용자 액션 로그에서 `split and zoomed plugin panes target an existing pane; use target_pane_id` 오류 확인. 실제 Herdr 0.9.0 서버에서도 split에 `--workspace`를 전달하면 같은 오류가 발생함을 재현.
- split 요청에서 `--workspace`를 제거하고, 액션 컨텍스트의 `focused_pane_id`를 대상 패널로 전달. 컨텍스트가 없을 때 환경 변수로 대체하며, 둘 다 없으면 실행 전에 설명이 있는 오류를 반환.
- 컨텍스트만 있는 백그라운드 액션, 오래된 환경 변수보다 현재 액션 컨텍스트 우선, 대상 ID 누락·빈 값·잘못된 JSON에 대한 회귀 테스트 추가.
- `tests/herdr_live.py`: 별도 XDG 폴더와 테스트 전용 서버에서 실제 액션 호출 후 같은 탭에 그래프 전용 패널 생성, 기존 포커스 유지, 터미널 버퍼의 `GIT GRAPH` 표시, 기존 전체 보기의 새 탭 생성 검증.

## v0.4.0 터미널 테마 연동 검증

2026-09-13 macOS Apple Silicon에서 `v0.4.0` 릴리스 빌드로 재검증했습니다. Rust 테스트 40개, 설치 테스트 6개, 포맷·Clippy, PTY·모의 그래픽 서버 및 별도 실제 Herdr 세션 검증을 통과했습니다.

- Herdr 0.9.0 소스의 OSC 10/11/4 응답 지원 확인. 별도 Herdr 세션에서 실제 플러그인의 ANSI 스냅샷을 읽어 조회 결과가 RGB 색상으로 반영되는 것 검증.
- 밝은/어두운 전경·배경과 ANSI 팔레트 응답, 1~4자리 RGB 채널, 잘못된 응답 무시, CLI와 환경 변수의 테마 우선순위 및 새 패널 전달 검증.
- PTY에서 BEL/ST 종료, 분할·지연 응답, 검색 입력과 Esc/Ctrl-C 보존, `r` 색상 재조회, 무응답 시 문자 모드와 터미널 기본색 유지 검증.
- PNG의 일반 배경과 미커밋 노드 내부 투명도, 문자 UI와 같은 선택 배경, 테마 변경 시 곡선 캐시 갱신 검증.
- 샘플 밝은/어두운 팔레트를 주입한 전체 화면·사이드바 SVG를 PNG로 렌더링해 시각 검토. 사용자 터미널의 실제 화면 캡처는 아님.
- `scripts/build.sh --debug`로 디버깅 심볼을 포함한 로컬 실행 파일을 생성하고, 별도 Herdr 세션에서 탭·사이드바 실행 확인.

## 터미널 연결 종료 수정 검증

2026-09-15 macOS Apple Silicon에서 검증했습니다.

- 수정 전 릴리스에서 PTY 연결 종료 후 CPU 약 96%, SIGTERM에 응답하지 않는 상태를 재현. 새 연결 종료 테스트가 기존 실행 파일에서 실패하는 것 확인.
- Crossterm 0.29.0의 Unix 입력 읽기가 EOF와 재시도할 수 없는 오류를 반환하도록 로컬 패치. 패치 범위와 원본 정보는 [vendor/README.md](../vendor/README.md)에 기록.
- 닫힌 stdout/stderr에 터미널 복구 오류를 출력하다가 다시 panic하는 경로를 제거. 입력 오류 및 정상 종료 모두 터미널 세션과 그래픽 스트림을 정리.
- 수정된 릴리스의 5초 유휴 CPU 약 0.2%, PTY 연결 종료 후 약 1ms 안에 종료 코드 0으로 종료하는 것 확인. 이 값은 해당 로컬 측정 결과이며 성능 보장치는 아님.
- `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, Rust 테스트 47개 통과.
- 별도 XDG 폴더와 서버를 사용하는 `tests/herdr_live.py` 통과. 실제 Herdr의 사이드바·중첩 분할·전체 보기·테마 응답 확인.
- `scripts/build.sh --release`로 로컬 실행 파일을 갱신하고 `bin/herdr-git-graph`와 릴리스 빌드의 SHA-256 일치 확인.

## 최소 Herdr 버전 재검증

2026-10-18 macOS Apple Silicon에서 현재 v0.5.0 소스를 빌드하고, [Herdr 공식 릴리스](https://github.com/herdrdev/herdr/releases)의 `herdr-macos-aarch64` 바이너리로 확인했습니다. 사용자 Herdr 설치와 세션은 변경하지 않았습니다.

| Herdr 버전 | `tests/herdr_live.py` 결과 |
| --- | --- |
| 0.8.0, 0.8.2 | 첫 사이드바 실행 실패. Herdr가 `./bin/herdr-git-graph`를 플러그인 루트가 아닌 대상 Git 저장소 아래에서 찾으며 `plugin_pane_open_failed` 반환 |
| 0.9.0, 0.9.1, 0.9.2, 0.9.3, 0.9.4, 0.9.5 | 전체 통과 |

- 버전 탐색은 임시 플러그인 디렉터리에서 `min_herdr_version`만 대상 버전으로 낮추고 동일한 실행 파일과 [실제 서버 통합 테스트](../tests/herdr_live.py)를 사용했습니다. 테스트마다 XDG 경로와 서버를 격리했습니다.
- 통과 범위: 액션·pane 등록, 사이드바 토글·재사용·빠른 탭 이동, 포커스와 pane/terminal ID 유지, 선택·스크롤·검색·너비 저장 및 탭 종료 후 복원, 좁은 탭에서 넓은 탭으로 너비 복원, 명시적 종료 후 자동 재생성 방지, 작업 공간 격리, 확대 상태의 이동 실패와 복구, 전체 보기, OSC 테마 응답.
- 공유 사이드바를 도입한 커밋 `5edbb49`에서 최소 버전을 0.9.0에서 0.9.5로 올렸지만, 사용하는 `tab.focused`, `tab.closed`, `pane.closed`, `pane.exited`는 [0.9.0 이벤트 정의](https://github.com/herdrdev/herdr/blob/v0.9.0/src/api/schema/events.rs)에 이미 있습니다. 실제 이동·복원 경로도 0.9.0에서 통과했으므로 **최소 버전을 0.9.0으로 되돌렸습니다.**
- 표준 Kitty graphics 지원도 0.9.2에 처음 생긴 것이 아닙니다. [0.9.0 pane terminal 소스](https://github.com/herdrdev/herdr/blob/v0.9.0/src/pane/terminal.rs)의 `kitty_graphics_write_requests_render_with_settle_backstop` 테스트는 pane PTY의 `a=T` 전송에 렌더링이 요청되는지 확인합니다. [0.9.2 릴리스](https://github.com/herdrdev/herdr/releases/tag/v0.9.2)는 기존 Herdr 전용 graphics API를 제거한 버전이며, 이 플러그인은 해당 API를 사용하지 않습니다.
- 수정한 실제 매니페스트로 0.9.0과 0.9.5 통합 테스트를 다시 통과했습니다. [CI](../.github/workflows/ci.yml)에도 두 버전을 고정해 macOS·Linux에서 검사하도록 추가했습니다. CI의 다운로드·실행 스크립트는 macOS에서 직접 실행해 통과했으며, 원격 CI와 Linux 결과는 아직 확인하지 않았습니다.
- `sh scripts/build.sh --debug`, Rust 테스트 88개, 설치 테스트 6개, `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `git diff --check` 통과.

실제 바깥 터미널의 픽셀 표시·곡선 정렬은 이번에 육안 검증하지 않았습니다. 위 결과는 headless 통합 테스트와 소스 확인 범위이며, 모든 터미널 조합에서 문제가 없다는 보장은 아닙니다.

## 미검증 범위

- 실제 Herdr 창과 바깥 터미널 조합에서의 픽셀 표시·스크롤 프레임률·원격 연결
- Linux에서의 실행 결과는 [GitHub Actions](https://github.com/sjlee06/herdr-git-graph/actions/workflows/ci.yml)에서 확인할 수 있습니다. 위 기록은 macOS 로컬 검증 기준입니다.
- checkout·merge·rebase·push 등 쓰기 동작: 현재 구현 범위에 포함하지 않습니다.

`preview*.svg`와 PNG는 현재 Ratatui UI와 tiny-skia 그래프 렌더러로 생성한 **샘플 스냅샷**입니다. 기본 사이드바와 전체 화면은 기존 내장 데모 이력을 유지하며, 커밋·파일 클릭 예시만 고정된 임시 Git 저장소의 실제 파일 목록과 패치를 Git worker로 불러옵니다. 기본 사이드바는 선택·강조 행이 없고, 커밋 선택 화면에는 변경 파일 목록, 파일 선택 화면에는 해당 파일의 diff가 표시됩니다. 실행 중인 Herdr 창을 캡처한 이미지는 아닙니다. `preview-text`는 같은 내장 데모 전체 화면의 문자 그래프 버전입니다.

`preview-sidebar`는 원래 캡처한 Herdr 작업 공간 프레임에 현재 기본 사이드바를 합성한 이미지입니다. 편집기와 테스트 출력은 새로 캡처한 내용이 아닙니다. [이미지 생성 안내](DEVELOPMENT.md#preview-assets)에 전체 재생성 절차와 상태별 파일을 정리했습니다.
