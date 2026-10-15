# 인덱싱 속도: debug vs release (M2 후속, PR #2 비고 1)

**결론: release 빌드는 debug보다 약 12배 빠르다(118개 파일 17.2s → 1.4s). 차이의 대부분은 PDF 텍스트 추출이다(23배).**
PR #2 실기 테스트의 "50초/118파일"은 debug 빌드 수치이므로 성능 판단에 쓰지 않는다. 앞으로 속도는 항상 release(`cargo build --release`)로 잰다.

## 측정 환경

- 2026-10-15, 클라우드 VM: Intel Xeon 2.10GHz 4코어, Linux, 로컬 디스크. `cargo build --release -p tidy-cli` (thin LTO, codegen-units=1).
- 실기(Windows) 바탕화면과 같은 구성의 **합성 코퍼스** 118개를 만들어 측정했다(실제 파일은 개인 자료라 쓰지 않음). 생성 스크립트: `scripts/bench/gen_corpus.py` (reportlab, openpyxl, python-docx, 나눔고딕 TTF 필요).

| 종류 | 개수 | 내용 |
|---|---|---|
| pdf | 20 | 3~30쪽, 한글 TTF 임베드(Identity-H) |
| xlsx | 20 | 200~3,000행 |
| docx | 15 | 50~400문단 |
| pptx | 6 | 테스트 fixture 복사 |
| txt / csv / html | 10 / 5 / 5 | 20~20,000줄 |
| zip / lnk / hwp / exe | 23 / 10 / 2 / 1 | 본문 미지원(메타만) |
| mp4 | 1 | 210MB(크기 상한 초과, 메타만) |

결과 분포는 실기와 같다: **text ok 81 / failed 0 / unsupported 36 / too big 1**.

## 결과

| 측정 | debug | release |
|---|---|---|
| 118개 최초 인덱싱 | 17.2 s | **1.4 s** |
| 118개 재실행(변경 없음) | 0.03 s | 0.02 s |
| 1,170개 최초 인덱싱(위 코퍼스 ×10, mp4 제외) | — | 15.4 s (≈76 파일/s, DB 127MB) |

종류별(같은 코퍼스, `--rebuild`로 종류별 폴더만 인덱싱):

| 종류 | 개수(총 크기) | debug | release |
|---|---|---|---|
| pdf | 20 (1.1MB) | 13.8 s | 0.6 s |
| xlsx | 20 (1.1MB) | 2.2 s | 0.3 s |
| docx | 15 (0.7MB) | 0.3 s | 0.1 s |
| txt / csv | 15 (1.8MB) | 0.4 s | 0.2 s |
| pptx / html / zip | 34 | 0.0 s | 0.0 s |

- 실기 50초와 합성 17초(debug)의 차이는 실제 PDF·xlsx가 더 크고 복잡하기 때문으로 보인다. 같은 비율이면 실기 release는 **4~5초** 정도로 예상된다. Windows에서 release로 다시 재서 이 표에 추가할 것:
  `cargo build --release -p tidy-cli` → `target\release\tidy.exe index <바탕화면> --no-recursive --rebuild`
- 규모는 거의 선형이다(×10 파일 → ×11 시간). 10만 파일 초기 인덱싱은 같은 구성이면 20~30분 걸린다. 백그라운드·저우선 처리(SPEC §5.5)가 필요한 근거다.
- DB 크기는 본문이 많은 문서 기준 파일당 약 110KB다(trigram + unicode61 두 FTS 테이블 포함, docs/fts-tokenizer.md).

## 발견: CID 폰트 한국어 PDF

reportlab의 내장 CID 폰트(`HYSMyeongJo-Medium`, 인코딩 `UniKS-UCS2-H`)로 만든 PDF는 `pdf-extract` 0.12가 **panic**한다(`unsupported encoding UniKS-UCS2-H`). 추출기 쪽 `catch_unwind` 덕에 `text_status = failed`로 기록되고 메타 인덱싱은 유지되지만, CLI 출력에 panic 메시지가 섞인다.
한글·Word·브라우저가 만드는 일반 PDF는 TTF 임베드(Identity-H)라 해당하지 않는다(실기 failed 0건). 같은 증상이 실제 문서에서 보이면 추출기 교체(예: `lopdf` 직접 파싱 또는 외부 `pdftotext` 폴백)를 검토한다.
