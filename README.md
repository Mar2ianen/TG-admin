# TG-admin

Текущий статус и архитектурные документы:

- [docs/IMPLEMENTATION_SUMMARY.md](/home/arch/Документы/Teloxide/docs/IMPLEMENTATION_SUMMARY.md:1) — что реально собрано в коде сейчас
- [docs/MVP_ROADMAP.md](/home/arch/Документы/Teloxide/docs/MVP_ROADMAP.md:1) — путь к MVP после фиксации новой routing model
- [docs/ROUTING_DIFF.md](/home/arch/Документы/Teloxide/docs/ROUTING_DIFF.md:1) — разрыв между target routing model и текущим кодом
- [docs/CLEAN_CODE_REFACTOR_PLAN.md](/home/arch/Документы/Teloxide/docs/CLEAN_CODE_REFACTOR_PLAN.md:1) — безопасный refactor plan под indexed/bucketed router

Коротко:

- в коде уже есть strong built-in moderation slice
- текущий рабочий built-in path проходит через `ModerationEngine`, но полного runtime router пока нет
- built-in command scope в коде: `/warn`, `/mute`, `/ban`, `/del`, `/undo`, `/msg`
- целевая routing model разделяет ingress classes (`realtime | recovery | scheduled | manual`), update traits и отдельный command index
- целевая модель — не плоский перебор handlers, а indexed/bucketed routing через `classify -> dispatch set -> execution lane`
