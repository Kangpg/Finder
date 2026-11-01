# Finder 구조와 검색 알고리즘

Kangpg · Finder 0.1.0 · Rust / Windows x64 · 2026-10-29 소스 기준

## 이미지

| 그림 | PNG · 2배 해상도 | 수정 가능한 벡터 | 오프라인 HTML |
| --- | --- | --- | --- |
| 시스템 구성도 | [3200 × 2160](finder-architecture.png) | [SVG](finder-architecture.svg) | [HTML](finder-architecture.html) |
| 검색 알고리즘 | [3200 × 2400](finder-search-algorithm.png) | [SVG](finder-search-algorithm.svg) | [HTML](finder-search-algorithm.html) |

HTML의 인라인 SVG가 편집 원본입니다. 네트워크 요청·외부 폰트·스크립트를 사용하지 않습니다. Windows의 Segoe UI / 맑은 고딕을 우선 사용하며, 다른 환경에서는 시스템 글꼴로 대체됩니다. 그림은 핵심 데이터 흐름을 압축한 설명으로, 모든 모듈이나 동기화 지점을 표시하지 않습니다.

## 구조 읽기

1. **수집**: NTFS 모드는 `FSCTL_ENUM_USN_DATA`로 MFT를 열거하고 `FSCTL_READ_USN_JOURNAL`로 변경분을 읽습니다. 폴더 모드는 WalkDir로 선택한 트리를 탐색하며 갱신 시 재탐색합니다. 일반 색인은 USN 저널 설정을 변경하지 않습니다.
2. **게시**: 색인 작업자가 변경을 적용한 `Arc<Index>`와 revision을 게시합니다. 검색은 보유한 스냅샷을 계속 읽을 수 있습니다. 공유 포인터 교환에는 짧은 Mutex 구간이 있으므로 lock-free 구조를 의미하지 않습니다.
3. **검색**: 좌우 창에는 독립 검색 요청 경로가 있습니다. 최신 대기 요청을 유지하고 epoch로 오래된 요청의 결과 게시를 차단합니다. 기본 검색 풀은 4스레드, 색인 풀은 2스레드이며 분리 색인도 같은 풀 예산을 공유합니다.
4. **표시**: egui 가상 목록이 결과를 그립니다. 아이콘·크기·수정 시각은 별도 STA 작업자가 보이는 행을 대상으로 조회합니다. worker 캐시는 창당 512개, TTL 10초이며 revision 변경 시 무효화합니다. UI 텍스처 보관과 조회값의 최신 여부는 별개입니다.
5. **Windows 통합**: 원본 아이콘과 SVN 오버레이, 클래식 메뉴·속성창·파일 작업은 Windows Shell을 사용합니다. 외부 Shell 확장 자체의 지연은 앱이 강제로 취소할 수 없습니다. 설정 저장 등 서비스 작업자, 트레이와 RegisterHotKey 처리는 개요에서 묶거나 생략했습니다.
6. **저장**: `.reaidx`는 bincode 직렬화와 BLAKE3 검증을 사용하는 로컬 스냅샷입니다. 역색인은 파일에 직렬화하지 않고 로드 후 재구성합니다.

## 검색 알고리즘 읽기

예시 `serverconfig.xml \global_tru`는 파일명 `serverconfig.xml`과 경로 `\global_tru`의 **AND** 조건입니다.

- **파싱**: AND / OR / NOT / 그룹 / 종류·확장자·추가 필터를 검색식 트리로 만듭니다. `\`가 포함된 토큰은 경로 조건으로 처리합니다.
- **3글자 역색인**: 기본 대소문자 구분 없는 파일명 검색은 소문자 이름을 Unicode scalar 3개씩 나눠 해시한 posting을 사용합니다. 예를 들어 `server`에는 `ser`, `erv`, `rve`, `ver`가 있습니다. 해당 파일명 ID 목록을 가리키는 posting 중 가장 작은 안전한 목록을 고릅니다. 모든 gram의 교집합을 계산하는 방식은 아닙니다.
- **조합**: AND는 안전한 후보 중 가장 작은 것을 선택합니다. OR는 모든 분기에 안전한 후보가 있을 때 합집합을 만들며, 그렇지 않으면 전체 검사로 돌아갑니다. 이름이 바뀌거나 새로 생긴 ID를 기본 후보에 합치고 삭제 ID를 제외합니다.
- **정확 검사**: 원래 검색식으로 파일명·경로·필터를 다시 확인합니다. 일반 문자열은 `memchr::memmem::find`, 정규식·와일드카드·단어 옵션은 컴파일된 패턴을 사용합니다. 해시 충돌은 후보를 늘릴 수 있지만 이 검사로 제거됩니다.
- **대체 경로**: 짧은 검색어·경로·NOT·정규식 등은 단독으로 후보를 제한하지 못할 수 있습니다. 대소문자 구분 모드, 역색인 미준비, 메모리 예산 초과·너무 많은 후보도 전체 검사를 선택할 수 있습니다. AND 안에 쓸 수 있는 긍정 파일명 조건이 있으면 그 조건으로 줄일 수 있습니다.
- **정렬**: 희소한 이름순 후보는 병렬 검사 후 `select_nth_unstable_by`로 상위 K개를 남겨 정렬합니다. 준비된 이름·위치 순서를 재사용하는 경로와 제한된 BinaryHeap으로 상위 K개를 모으는 경로도 있습니다. 전체 일치 개수를 계산하되 GUI 표시 결과는 최대 5,000개입니다. 이는 전체 색인 레코드 수 제한이 아닙니다.

이 그림은 파일명·경로 검색을 설명합니다. 파일 내용 전문 검색이나 네트워크 검색 서버를 구현했다는 의미가 아닙니다. 성능 실측 결과도 아니며 실제 작업량은 질의 선택도, 정렬에 필요한 메타데이터, 색인 모드와 Shell 확장에 따라 달라집니다.

## 코드 근거

| 책임 | 소스 |
| --- | --- |
| NTFS / 폴더 수집 | [ntfs.rs](../../src/ntfs.rs), [usn.rs](../../src/usn.rs), [folder.rs](../../src/folder.rs) |
| 작업 큐 / 스냅샷 / epoch | [background.rs](../../src/background.rs) |
| 레코드 / 검색 / 정렬 | [index.rs](../../src/index.rs), [ordering.rs](../../src/ordering.rs), [locations.rs](../../src/locations.rs) |
| 검색식 / 역색인 | [query.rs](../../src/query.rs), [trigram.rs](../../src/trigram.rs) |
| UI / 표시 정보 | [gui.rs](../../src/gui.rs), [search_pane.rs](../../src/search_pane.rs), [row_metadata.rs](../../src/row_metadata.rs) |
| Shell / 서비스 / 상주 | [shell_menu.rs](../../src/shell_menu.rs), [fileops.rs](../../src/fileops.rs), [services.rs](../../src/services.rs), [resident.rs](../../src/resident.rs) |
| 저장 / 출력 장치 | [storage.rs](../../src/storage.rs), [graphics.rs](../../src/graphics.rs) |

문서 수정 시 HTML을 먼저 고친 뒤 SVG와 PNG를 다시 내보내 세 형식을 일치시킵니다. 소스 내보내기는 `docs`의 Markdown·HTML·SVG·PNG도 포함하지만 실행 파일·빌드 산출물·사용자 데이터는 계속 제외합니다.
