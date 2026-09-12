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
- `cargo test --locked`: 단위 테스트 9개 + 실제 Git 저장소 통합 테스트 4개
- `scripts/build.sh`: 최적화한 macOS arm64 실행 파일 생성
- 병합 그래프의 각 outgoing edge가 실제 부모 OID와 일치하는지 검증
- 여러 부모 병합, 연결되지 않은 이력, 페이지 밖 부모, 빈 저장소, 빈 커밋 제목
- 한글 메시지와 검색, detached HEAD, linked worktree, shallow clone, 브랜치 필터
- 실제 diff 내용, 조회 전후 working tree 상태 일치, external diff 비활성화
- 여러 터미널 크기 및 도움말 화면을 Ratatui TestBackend로 렌더링
- `tests/smoke.py`: PTY에서 탐색, 한글 검색, 도움말, 크기 변경, 정상 종료
- SIGTERM 종료 시 raw mode·alternate screen·마우스 캡처 복구
- 가짜 Herdr 서버와 실제 실행 파일 간 그래픽 협상, PNG 프레임 전송, 크기 변경
- 도움말 표시 및 종료 시 그래픽 스트림/레이어 수명 종료
- 그래픽 API가 `feature_disabled`를 반환할 때 문자 모드 전환
- Herdr 0.9.0 CLI로 매니페스트를 읽고 `plugin_linked` 응답 확인
- 같은 Ratatui 화면 버퍼에서 만든 SVG와 tiny-skia PNG를 시각적으로 검토

매니페스트 검증은 별도 작업 폴더의 XDG 설정·상태 디렉터리와 비활성 플러그인 등록으로 수행했습니다.

## v0.1.1 설치 수정 검증

- [릴리스 워크플로](https://github.com/sjlee06/herdr-git-graph/actions/runs/34683222669): macOS·Linux 각각 arm64/x86_64의 네 환경에서 빌드, Rust 테스트, PTY·그래픽 모의 서버 테스트 통과.
- `tests/install.py`: 운영체제별 파일 선택, 체크섬 검증, 다운로드 실패, 기존 파일 보존 등 설치 테스트 6개 통과.
- Cargo가 없는 `PATH`를 사용해 실제 공개 GitHub 저장소에 `herdr plugin install sjlee06/herdr-git-graph --yes`를 실행하고, 이전 v0.1.0 설치가 v0.1.1로 교체되는 것 확인.
- 설치 검증은 별도 XDG 설정·상태 폴더에서 수행했으며, 사용자 설정은 변경하지 않았습니다.

## 미검증 범위

- 실제 Herdr 창과 바깥 터미널 조합에서의 픽셀 표시·스크롤 프레임률·원격 연결
- Linux에서의 실행 결과는 [GitHub Actions](https://github.com/sjlee06/herdr-git-graph/actions/workflows/ci.yml)에서 확인할 수 있습니다. 위 기록은 macOS 로컬 검증 기준입니다.
- checkout·merge·rebase·push 등 쓰기 동작: 현재 구현 범위에 포함하지 않습니다.

`preview.svg`는 실제 Ratatui 화면 버퍼에 같은 tiny-skia 그래프 렌더러를 합성한 **데모 스냅샷**입니다. 실행 중인 Herdr 창을 캡처한 이미지는 아닙니다. `preview-text.svg`는 문자 모드의 같은 화면입니다.
