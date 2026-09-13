# Herdr Git Graph

**코드 옆에 Git 이력을 두고, diff가 필요할 때 전체 화면으로 살펴보세요.**

[![CI](https://github.com/sjlee06/herdr-git-graph/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/sjlee06/herdr-git-graph/actions/workflows/ci.yml)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Herdr Plugin](https://img.shields.io/badge/Herdr-plugin-5eead4)](https://herdr.dev/docs/plugins/)

[English](README.md) · 한국어

Herdr용 조회 전용 Git 그래프 플러그인입니다. 사이드바에서 이력을 살펴보고, 전체 화면에서 변경 내용을 확인할 수 있습니다.

<p align="center">
  <img src="docs/preview-sidebar.png" width="1600" alt="Herdr 코드 편집기 옆에서 데모 이력을 표시하는 Git Graph 사이드바">
</p>

*코드 옆에서 Git 이력을 확인하는 사이드바 데모입니다.*

## 주요 기능

- **브랜치·병합 그래프:** 분기별 색상과 브랜치·태그 표시.
- **Diff 보기:** 전체 화면에서 커밋과 스테이징·미스테이징 변경 내용 확인.
- **검색:** 메시지·작성자·ref·해시로 커밋 검색.
- **자동 갱신:** 기본 2초 간격으로 로컬 변경 반영.
- **터미널 연동:** 터미널 테마를 자동 적용하고, 호환되는 Herdr 패널에서는 곡선을, 그 외에는 문자 그래프를 표시.

## 설치

**Herdr 0.9.0 이상**, **Git 2.31 이상**, `curl`이 필요합니다. **Rust나 Cargo는 설치하지 않아도 됩니다.**

배포 실행 파일은 **macOS 11 이상**, **glibc 2.35 이상인 Linux**(예: Ubuntu 22.04 이상)의 Apple Silicon/ARM64 및 x86_64를 지원합니다. 다른 환경에서는 [소스 빌드 안내](docs/DEVELOPMENT.md)를 참고하세요.

```bash
herdr plugin install sjlee06/herdr-git-graph
```

Git 저장소가 열려 있는 Herdr 작업 공간에서 현재 패널 오른쪽에 사이드바를 엽니다.

```bash
herdr plugin action invoke herdr.git-graph.sidebar
```

분할선을 드래그하거나 `prefix+r`로 크기를 바꾸세요. `prefix+l`로 사이드바에 이동한 뒤 `q`로 닫을 수 있습니다.

### 전체 화면

![브랜치 필터, 곡선 커밋 이력, 컬러 diff 상세 패널을 표시한 전체 데모 화면](docs/preview.png)

*브랜치를 필터링하고 커밋 diff를 확인하는 전체 화면 데모입니다.*

브랜치별 이력을 살펴보거나 커밋 diff를 확인할 때는 새 탭으로 전체 그래프를 엽니다.

```bash
herdr plugin action invoke herdr.git-graph.open
```

### 단축키 연결

`~/.config/herdr/config.toml`에 사용하지 않는 키를 등록합니다.

```toml
[[keys.command]]
key = "prefix+u"
type = "plugin_action"
command = "herdr.git-graph.open"
description = "Open Git Graph"

[[keys.command]]
key = "prefix+shift+u"
type = "plugin_action"
command = "herdr.git-graph.sidebar"
description = "Open Git Graph Sidebar"
```

`herdr config check`로 충돌을 확인하고 `herdr server reload-config`를 실행하세요. 설정한 prefix 다음 `u`는 전체 그래프, `Shift+u`는 사이드바를 엽니다.

## 사용법

전체 화면에서 브랜치를 선택하고 **Enter**를 누르면 해당 이력만 표시하며, **a**를 누르면 전체 브랜치로 돌아갑니다. 커밋이나 **Uncommitted changes**를 선택하면 아래 상세 패널에서 diff를 확인할 수 있습니다.

| 키 | 동작 |
| --- | --- |
| `Tab` / `Shift-Tab` | 전체 화면에서 패널 전환 |
| `↑` `↓` / `j` `k` | 항목 이동 또는 diff 스크롤 |
| `Enter` | 검색 적용; 전체 화면에서는 브랜치 필터 적용 / 상세 패널 이동 |
| `/`, `n` / `N` | 검색, 다음 / 이전 검색 결과 |
| `a` | 전체 브랜치 보기 |
| `r` | 로컬 이력 새로고침 |
| `?` | 도움말 열기 / 닫기 |
| `q` / `Ctrl-C` | 종료 (`q`는 도움말부터 닫기) |

마우스 클릭으로 항목을 선택하고 포인터 아래 패널을 휠로 스크롤할 수 있습니다.

로컬 Git 데이터를 조회하므로 원격의 새 커밋은 기존 Git 작업 흐름에서 fetch한 뒤 확인할 수 있습니다. 검색은 불러온 이력을 대상으로 하며, **기본 2,000개 커밋**을 `--limit`으로 조정할 수 있습니다.

전체 단축키, 저장소 지정, 테마 설정, 문제 해결은 [상세 사용 안내](docs/USAGE.md)를 참고하세요. [Herdr 없이 실행](docs/USAGE.md#run-without-herdr)할 수도 있습니다.

## 개발 및 기여

빌드, 테스트, 기여 방법은 [개발 안내](docs/DEVELOPMENT.md)를 참고하세요. 버그 제보에는 OS, Herdr 버전, 터미널 종류, 재현 방법을 함께 적어주세요.

## 라이선스

[MIT](LICENSE). [Herdr](https://herdr.dev/)용 독립 커뮤니티 플러그인입니다.
