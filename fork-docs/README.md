# fork-docs

이 fork(`codex-fork`)에서 upstream(`openai/codex`)과 다르게 가져가는 결정·작업 기록을 모아두는 폴더입니다.

upstream 공통 문서는 `/docs` 에 있고, 이 폴더는 **이 fork 한정**의 변경/운영 가이드만 담습니다.

## Index

| 문서 | 내용 |
| --- | --- |
| [`ollama-migration.md`](./ollama-migration.md) | LLM 호출을 OpenAI → 원격 Ollama (`qwen3.5-122b`)로 전환하는 작업 계획·변경 지점·검증 절차 |

## 새 문서를 추가할 때

- 파일명은 `kebab-case.md`.
- 문서 첫 부분에 **Context** (왜 만드는지) 와 **변경 지점** (어느 파일·어떤 선) 을 명시.
- upstream PR 로 보낼 가능성이 있으면 그 사실도 적어두기 — fork 한정인지 upstream 후보인지 구분 필요.
- 이 README 의 Index 표에 한 줄 추가.
