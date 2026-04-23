# MVP Status

Короткий статус против [docs/MVP_ROADMAP.md](/home/arch/Документы/Teloxide/docs/MVP_ROADMAP.md:1).

## Verdict

Текущий код уже проходит MVP-gate для built-in moderation runtime.

Это подтверждается не одним модульным слоем, а текущим runtime path:

- `main -> Application -> Runtime` уже является рабочим composition root: [src/main.rs](/home/arch/Документы/Teloxide/src/main.rs:1), [src/app.rs](/home/arch/Документы/Teloxide/src/app.rs:1), [src/runtime.rs](/home/arch/Документы/Teloxide/src/runtime.rs:1)
- live polling ingress и live `teloxide-core` transport поднимаются из runtime при наличии `bot_token`: [src/runtime.rs](/home/arch/Документы/Teloxide/src/runtime.rs:118), [src/tg.rs](/home/arch/Документы/Teloxide/src/tg.rs:1), [docs/RUNTIME_CONTRACT.md](/home/arch/Документы/Teloxide/docs/RUNTIME_CONTRACT.md:1)
- runtime-level `normalize -> classify -> dispatch -> execute` уже проходит через `IngressPipeline` и `ExecutionRouter`: [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:111), [src/router.rs](/home/arch/Документы/Teloxide/src/router.rs:1)

## MVP Gates

- `runtime поднимается из main` — выполнено. Entry path идет через `Application::from_config(...).run()` и дальше в `Runtime::startup()/run_until_shutdown()`: [src/main.rs](/home/arch/Документы/Teloxide/src/main.rs:7), [src/app.rs](/home/arch/Документы/Teloxide/src/app.rs:17)
- `есть реальный Telegram ingestion loop` — выполнено. `IngressPipeline::run_until_shutdown()` читает live updates, а runtime поднимает polling bot только при валидном `bot_token`: [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:63), [src/runtime.rs](/home/arch/Документы/Teloxide/src/runtime.rs:132)
- `есть startup build routing indexes` — выполнено. Runtime собирает `RouterIndex::from_registry(&self.registry)` на startup: [src/runtime.rs](/home/arch/Документы/Teloxide/src/runtime.rs:51)
- `есть runtime-level event classification` — выполнено. Router уже строит `dispatch set` из ingress/update traits/command index, а не делает плоский глобальный scan: [src/router.rs](/home/arch/Документы/Teloxide/src/router.rs:187), [docs/IMPLEMENTATION_SUMMARY.md](/home/arch/Документы/Teloxide/docs/IMPLEMENTATION_SUMMARY.md:1)
- `есть dispatch set -> execution lane path` — выполнено. Для built-in moderation есть end-to-end execution lane; для units уже есть manifest-aware dispatch envelope: [src/router.rs](/home/arch/Документы/Teloxide/src/router.rs:418), [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:1133)
- ``/warn`, `/mute`, `/del`, `/undo` работают на живом transport`` — выполнено на runtime-level ingress tests: [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:1319), [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:1401), [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:1534), [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:1583)
- `audit и replay safety работают не только в модульных тестах` — выполнено. Replay проверяется через `IngressPipeline::process_update`, а built-in e2e tests проверяют запись audit/state effects: [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:1205), [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:1319), [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:1583)
- `dry-run остается предсказуемым` — выполнено. Runtime-level ingress coverage есть для live dry-run moderation path: [src/ingress.rs](/home/arch/Документы/Teloxide/src/ingress.rs:1463), commit `cfbfe5f`

## Not Blocking MVP

- Полный scriptable/unit runtime beyond routing envelope еще не собран end-to-end. Для MVP это не блокер, потому что текущий живой execution gate уже закрыт built-in moderation lane, а manifests уже участвуют в routing/indexing: [src/router.rs](/home/arch/Документы/Teloxide/src/router.rs:418), [docs/MVP_ROADMAP.md](/home/arch/Документы/Teloxide/docs/MVP_ROADMAP.md:1)
- Полный hot reload manifests тоже не обязателен для этого gate. Сам roadmap отдельно относит полный reload после MVP: [docs/MVP_ROADMAP.md](/home/arch/Документы/Teloxide/docs/MVP_ROADMAP.md:1)
- Остаточные риски уже операционные, а не MVP-blocking: capability coverage для новых commands/units, более широкий live update surface и дальнейший hardening scriptable path.
