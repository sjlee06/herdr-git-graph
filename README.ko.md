# Herdr Git Graph

**Herdr 안에서 Git 브랜치, 병합 이력, 커밋 diff를 탐색하세요.**

[![CI](https://github.com/sjlee06/herdr-git-graph/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/sjlee06/herdr-git-graph/actions/workflows/ci.yml)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Herdr Plugin](https://img.shields.io/badge/Herdr-plugin-5eead4)](https://herdr.dev/docs/plugins/)

[English](README.md) · 한국어

Rust + Ratatui로 만든 조회 전용 Git 그래프 플러그인입니다. 로컬·원격 추적 브랜치와 병합 이력을 살펴보고, 커밋을 검색하고, 변경 내용을 확인할 수 있습니다. 호환되는 Herdr 패널에서는 부드러운 곡선을, 일반 터미널에서는 컬러 Unicode 그래프를 표시합니다.

![브랜치 목록, 곡선 커밋 그래프, 상세 패널](docs/preview.png)

*앱의 Ratatui 화면과 곡선 렌더러로 생성한 데모 스냅샷입니다. [문자 모드 미리보기](docs/preview-text.svg).*

## 주요 기능

- **브랜치·병합 그래프:** 분기별 색상, 브랜치·태그 표시, detached HEAD와 linked worktree 지원.
- **커밋 상세 보기:** 메타데이터, 변경 파일 통계, 컬러 diff를 한 화면에서 확인.
- **검색과 탐색:** 메시지·작성자·ref·해시 검색, 키보드·마우스 조작.
- **부드러운 곡선:** Herdr 그래픽 API를 통한 베지어 곡선 출력과 일반 터미널용 문자 그래프.

## 설치

**Herdr 0.9.0 이상**, **Git 2.31 이상**, `curl`이 필요합니다. **Rust나 Cargo는 설치하지 않아도 됩니다.** 설치 스크립트가 환경에 맞는 [릴리스 실행 파일](https://github.com/sjlee06/herdr-git-graph/releases)을 내려받고 `shasum` 또는 `sha256sum`으로 SHA-256 체크섬을 검증합니다.

배포 실행 파일은 **macOS 11 이상**, **glibc 2.35 이상인 Linux**(예: Ubuntu 22.04 이상)의 Apple Silicon/ARM64 및 x86_64를 지원합니다. 다른 환경에서는 [소스 빌드 안내](docs/DEVELOPMENT.md)를 참고하세요.

```bash
herdr plugin install sjlee06/herdr-git-graph
```

Git 저장소가 열려 있는 Herdr 작업 공간에서 실행하면 새 탭으로 그래프를 엽니다.

```bash
herdr plugin action invoke herdr.git-graph.open
```

특정 저장소를 직접 지정할 수도 있습니다.

```bash
herdr plugin pane open \
  --plugin herdr.git-graph \
  --entrypoint graph \
  --cwd /path/to/repository \
  --focus
```

### 단축키 연결

`~/.config/herdr/config.toml`에 사용하지 않는 키를 등록합니다.

```toml
[[keys.command]]
key = "prefix+g"
type = "plugin_action"
command = "herdr.git-graph.open"
description = "Open Git Graph"
```

`herdr server reload-config`를 실행한 뒤, 설정한 prefix를 누르고 `g`를 누릅니다.

## 사용법

브랜치를 선택하고 **Enter**를 누르면 해당 브랜치의 이력만 표시합니다. **All branches**를 선택하거나 **a**를 누르면 전체 이력으로 돌아갑니다. 커밋을 선택하면 아래 상세 패널에서 변경 내용을 확인할 수 있습니다.

| 키 | 동작 |
| --- | --- |
| `Tab` / `Shift-Tab` | 패널 전환 |
| `↑` `↓` / `j` `k` | 항목 이동 또는 diff 스크롤 |
| `Enter` | 브랜치 필터 적용 / 상세 패널 이동 |
| `/`, `n` / `N` | 검색, 다음 / 이전 검색 결과 |
| `a` | 전체 브랜치 보기 |
| `r` | 로컬 이력 새로고침 |
| `d` | 상세 패널 표시 / 숨기기 |
| `?` | 전체 단축키 보기 |
| `q` / `Ctrl-C` | 종료 |

마우스 클릭으로 항목을 선택하고 포인터 아래 패널을 휠로 스크롤할 수 있습니다. 전체 단축키, 렌더러 설정, 문제 해결은 [상세 사용 안내](docs/USAGE.md)를 참고하세요.

로컬 Git 데이터를 조회합니다. 원격의 새 커밋은 기존 Git 작업 흐름에서 fetch한 후 `r`로 반영하세요. fetch·checkout·commit·merge·rebase·push는 실행하지 않습니다. 검색은 불러온 이력을 대상으로 하며, **기본 2,000개 커밋**을 `--limit`으로 조정할 수 있습니다.

## Herdr 없이 실행

```bash
git clone https://github.com/sjlee06/herdr-git-graph.git
cd herdr-git-graph
sh scripts/install.sh

./bin/herdr-git-graph --demo
./bin/herdr-git-graph --repo /path/to/repository
```

일반 터미널에서는 문자 그래프를 사용합니다. 픽셀 곡선은 호환되는 Herdr 패널과 바깥 터미널이 필요합니다. [렌더러 안내](docs/USAGE.md#renderers)에서 설정과 문자 모드 전환 조건을 확인할 수 있습니다.

## 개발 및 기여

로컬 플러그인 연결, 빌드, 테스트, 코드 구조는 [개발 안내](docs/DEVELOPMENT.md)에 정리했습니다. [검증 기록](docs/VALIDATION.md)에는 자동화 테스트와 실제 터미널 화면 검증의 범위를 구분해 두었습니다.

버그 제보와 기여를 환영합니다. 이슈를 등록할 때 OS, Herdr 버전, 터미널 종류, 재현 방법을 함께 적어주세요.

## 라이선스

[MIT](LICENSE). [Herdr](https://herdr.dev/)용 독립 커뮤니티 플러그인입니다.
