# TG-admin

Текущий статус и архитектурные документы:

- [docs/IMPLEMENTATION_SUMMARY.md](/home/arch/Документы/Teloxide/docs/IMPLEMENTATION_SUMMARY.md:1) — что реально собрано в коде сейчас
- [docs/ML_SERVER_CONTRACT.md](/home/arch/Документы/Teloxide/docs/ML_SERVER_CONTRACT.md:1) — typed contract layer для интеграции с локальным `ml-server`
- [docs/RUNTIME_CONTRACT.md](/home/arch/Документы/Teloxide/docs/RUNTIME_CONTRACT.md:1) — текущий runtime contract и что реально читает/использует startup path
- [docs/MVP_ROADMAP.md](/home/arch/Документы/Teloxide/docs/MVP_ROADMAP.md:1) — путь к MVP после фиксации новой routing model
- [docs/MVP_STATUS.md](/home/arch/Документы/Teloxide/docs/MVP_STATUS.md:1) — краткая сверка текущего кода против MVP-gate из roadmap
- [docs/ROUTING_DIFF.md](/home/arch/Документы/Teloxide/docs/ROUTING_DIFF.md:1) — разрыв между target routing model и текущим кодом
- [docs/CLEAN_CODE_REFACTOR_PLAN.md](/home/arch/Документы/Teloxide/docs/CLEAN_CODE_REFACTOR_PLAN.md:1) — безопасный refactor plan под indexed/bucketed router
- [docs/PLUGIN_AUTHORING_GUIDE.md](/home/arch/Документы/Teloxide/docs/PLUGIN_AUTHORING_GUIDE.md:1) — как писать plugin/unit manifests и handlers под текущие контракты и будущие buckets

Коротко:

- в коде уже есть strong built-in moderation slice
- текущий runtime path уже включает `Runtime`, `ExecutionRouter`, `IngressPipeline`, polling ingest loop и live `teloxide-core` transport при наличии `bot_token`
- built-in moderation все еще остается основной реально работающей execution lane; unit-driven execution lane end-to-end еще не собран
- built-in command scope в коде: `/warn`, `/mute`, `/ban`, `/del`, `/undo`, `/msg`
- `ml_server.base_url` задает default live endpoint для `HostApi` ML transport; request-level `base_url` может его переопределять
- целевая routing model разделяет ingress classes (`realtime | recovery | scheduled | manual`), update traits и отдельный command index
- целевая модель — не плоский перебор handlers, а indexed/bucketed routing через `classify -> dispatch set -> execution lane`

Пример runtime-конфига: [config.example.toml](/home/arch/Документы/Teloxide/config.example.toml:1). Для точных startup/runtime semantics ориентир — [docs/RUNTIME_CONTRACT.md](/home/arch/Документы/Teloxide/docs/RUNTIME_CONTRACT.md:1).
