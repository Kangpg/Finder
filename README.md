# Finder

Windows에서 로컬 파일과 폴더를 이름으로 찾는 데스크톱 검색 프로그램입니다. Rust로 구현했으며 NTFS 드라이브 색인과 일반 폴더 색인을 지원합니다.

## 주요 기능

- **색인**: NTFS MFT 열거·USN 저널 변경 추적, 관리자 권한 없이 사용하는 폴더 탐색.
- **검색**: 검색어 조합·제외 조건, 확장자·파일 종류 필터, 와일드카드, 정규식, 전체 경로 검색.
- **분할 보기**: 두 검색창에서 색인을 공유하거나 서로 다른 폴더·드라이브 검색.
- **Windows 통합**: 기본 아이콘·Shell 메뉴, 파일 열기, 경로 복사, 이름 변경, 복사·이동.
- **편의 기능**: 최근 검색·즐겨찾기, 한국어·영어 UI, 트레이·전역 단축키, 자동 갱신.
- **CLI**: 색인 생성·검색·갱신·저장과 통계 조회.

## 시작하기

Windows 10 이상 x64를 대상으로 합니다. 빌드된 설치판을 실행하거나 포터블 ZIP을 풀고 `finder.exe`를 실행합니다. 실행 파일 사용에는 Rust가 필요하지 않습니다. 이 저장소에는 소스 코드만 포함합니다.

[v1.0.1 릴리즈](https://github.com/Kangpg/Finder/releases/tag/v1.0.1)에서 설치판과 포터블 ZIP을 받을 수 있습니다. 변경 내용은 [CHANGELOG.md](CHANGELOG.md)를 참고하세요.

1. **색인 관리**에서 폴더 또는 NTFS 드라이브를 선택합니다.
2. 검색창에 파일명이나 검색식을 입력합니다.
3. 더블클릭 또는 Enter로 결과를 열고, F5로 색인을 갱신합니다.

NTFS 드라이브 색인에는 볼륨 읽기 권한(일반적으로 관리자 권한)과 활성화된 USN 저널이 필요합니다. 권한이 없으면 폴더 모드를 사용하세요. 폴더 모드는 갱신 시 폴더 트리를 다시 읽습니다.

| 검색식 | 의미 |
| --- | --- |
| `report 한국` | 두 문자열 모두 포함 |
| `"annual report"` | 공백을 포함한 문자열 |
| `foo \| bar` | 둘 중 하나 포함 |
| `!temp` | temp를 포함하는 이름 제외 |
| `ext:rs;cpp` | 확장자로 검색 |
| `*.txt` | 와일드카드 검색 |
| `folder:` | 폴더만 표시 |
| `path:Projects` | 전체 경로로 검색 |

기본 창 열기 단축키는 **Ctrl+Alt+Space**입니다. 창을 닫으면 기본적으로 트레이에서 계속 실행하며, 완전히 종료하려면 트레이 메뉴의 **종료**를 선택합니다.

## 소스 빌드

Rust 1.95 이상과 MSVC C++ 빌드 도구가 필요합니다. `.cargo/config.toml`에서 Windows x64 MSVC 대상을 지정합니다.

```powershell
cargo build --release --locked
.\target\x86_64-pc-windows-msvc\release\finder.exe
```

CLI 예시:

```powershell
$app = '.\target\x86_64-pc-windows-msvc\release\finder-cli.exe'
& $app index --folder 'D:\Projects' --output '.\projects.reaidx'
& $app search --index '.\projects.reaidx' 'ext:rs;cpp !target' --json
& $app refresh --index '.\projects.reaidx'
```

## 구현과 지원 범위

NTFS 모드는 MFT에서 파일 정보를 수집하고 USN 저널로 변경을 반영합니다. 검색에는 Unicode trigram 보조 역색인을 사용하며, 적용할 수 없는 조건은 전체 검색으로 처리합니다. GUI는 egui/eframe 기반이며 색인·검색·파일 정보 조회를 백그라운드에서 처리합니다.

파일명·경로 검색을 지원하며 파일 내용·크기·날짜 조건 검색은 지원하지 않습니다. 목록에는 최대 5,000개 결과를 표시하고 전체 일치 개수는 별도로 계산합니다. 각 색인은 하나의 폴더 또는 NTFS 볼륨을 대상으로 합니다. ReFS, 모든 하드링크 이름 열거, 네트워크 공유의 실시간 추적은 지원하지 않습니다.

## 문서

- [사용법 및 구현 상세](docs/usage.md)
- [구조와 검색 알고리즘](docs/architecture/README.md)
- [개발 경과·이슈·비교 보고서](docs/reports/2026-10-29/README.md) — 해당 날짜의 기록과 검증 범위

## 검증과 배포

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

설치판·포터블 패키지는 `scripts/build-release.ps1`로 생성하며 NSIS 3.13이 추가로 필요합니다. 의존성의 라이선스·저작권 문서는 배포 시 `THIRD-PARTY-NOTICES.txt`로 함께 묶습니다.

제작·배포: **Kangpg**

