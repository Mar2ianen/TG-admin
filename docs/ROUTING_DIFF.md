# Routing Diff: Target Docs vs Current Code

## Цель

Зафиксировать точный разрыв между:

- целевой bucketed/indexed routing model
- текущим состоянием кода на `2026-04-22`

Это не redesign с нуля, а честная карта разрыва.

## Короткий вывод

В коде уже есть:

- `EventContext`
- typed parser/dispatch chain
- built-in moderation vertical slice
- replay/audit/storage contracts

Но в коде все еще нет:

- unit bootstrap из `config.paths.units_dir`
- actual unit execution lane
- полного live ingress context parity для admin/topic и части update types

То есть:

- routing fragments есть
- working moderation route есть
- indexed runtime path уже есть, но unit-aware execution и часть ingress context еще неполные

## Что считается целевой моделью

Документация теперь исходит из следующей модели:

1. любой внешний input сначала становится `EventContext`
2. runtime делает cheap classification события
3. router строит `dispatch set` релевантных buckets
4. исполняются только нужные handler groups
5. built-in и unit-driven execution выбираются как execution lanes

Ключевая поправка:

router не выбирает “одну корзину”.

Он строит набор релевантных индексов.

Пример:

- `text`
- `command`
- `reply`

Это уже три trait-а одного события, а не одна корзина.

## Что реально есть в коде

## 1. Верхний runtime path

`main` сейчас делает:

1. `AppConfig::load()`
2. `init_logging()`
3. `Application::from_config(config)`
4. `application.run().await`

`Application::startup()` теперь уже:

- bootstrap-ит storage
- fail-fast валидирует startup path
- делегирует startup в `Runtime`

`Runtime::startup()` уже:

- собирает `ExecutionRouter`
- строит `RouterIndex` из текущего registry
- при наличии `bot_token` поднимает `IngressPipeline`
- подключает polling ingest loop и live `teloxide-core` transport

Но все еще не делает:

- bootstrap units из `config.paths.units_dir`
- actual unit execution lane
- полный ingress context capture для admin/topic и всех declared update types

Вывод:

lifecycle shell и отдельный `Runtime` уже есть. Основной gap теперь в unit bootstrap/execution и неполном live ingress context.

## 2. Реальный working route в коде

Сейчас реально работает такой локальный built-in path:

`EventContext -> EventCommandDispatcher -> ParsedCommandLine -> ExpandedCommandLine -> ModerationEngine -> execute_*`

Именно он сейчас выполняет built-in moderation semantics.

То есть `ModerationEngine` сейчас является рабочим local executor/route для built-in moderation, но не top-level runtime router.

Вывод:

command routing внутри moderation есть, а runtime routing tree уже собран в минимальном виде через `ExecutionRouter`; основной пробел не в отсутствии router как такового, а в неполном unit-aware dispatch.

## 3. Telegram layer

Есть:

- typed request/result contract
- dry-run behavior
- idempotency shell

Нет:

- live transport без `bot_token`
- полный update/context coverage в ingress
- webhook path

Вывод:

Telegram execution contract, live `teloxide-core` transport и polling ingress path уже есть. Неполными остаются context mapping и coverage.

## 4. Unit layer

Есть:

- manifests
- validation
- registry
- safe reload semantics

Нет:

- startup bootstrap из `config.paths.units_dir`
- route envelope binding
- manifest-aware runtime dispatch
- actual unit execution lane

Вывод:

unit layer собран как data/config layer, но не как routing layer.

## 5. Host API layer

Есть:

- typed operation router
- structured validation
- fail-closed capability checks

Но built-in moderation flow все еще ходит напрямую в `storage`/`tg`, а не через единый policy/runtime surface.

Вывод:

Host API не является общим execution router и не образует отдельную полезную execution lane в текущей документации.

## Diff по ключевым зонам

| Область | Target docs | Current code | Статус |
| --- | --- | --- | --- |
| Runtime composition root | Должен собирать router, ingress и executors | `Runtime` уже собирает router, ingress и executors; `Application` остается lifecycle shell | Present |
| Event classification | Должен быть отдельный runtime шаг | Есть минимальный runtime path через ingress normalization и router classification | Partial |
| Bucket/index build | Должен строиться на startup/reload | `RouterIndex::from_registry` уже есть, но строится только из пустого runtime registry | Partial |
| Dispatch set routing | Должен выбирать релевантные bucket groups | `ExecutionRouter` уже есть, но unit-aware dispatch неполон | Partial |
| Built-in moderation path | Должен существовать как одна из execution lanes | Есть локально внутри `ModerationEngine` | Present |
| Command parser chain | Должен сохраняться как typed path | Есть | Present |
| Journaling | Должен быть ingress concern | Уже встроен в `IngressPipeline` для live polling path | Present |
| Dedupe/replay | Должен стоять до destructive effects | Уже стоит в `IngressPipeline` до router execution, но параллельно существует и moderation-level guard | Partial |
| Unit bootstrap | Должен грузиться на startup | В runtime не встроен | Missing |
| Event -> unit envelope | Должен быть явный binding | Для realtime path отсутствует | Missing |
| Built-in vs scriptable boundary | Должна быть явной execution lane boundary | Пока фактически built-in only | Partial |
| Live Telegram transport | Должен быть production transport | `teloxide-core` transport включается при наличии `bot_token`, иначе остается `noop` | Partial |

## Главное расхождение

Главный gap уже не формулируется как “нет decision tree вообще”.

Точнее формулировка такая:

- есть локальный moderation decision path
- есть runtime-level indexed path через `ExecutionRouter` и live ingress
- нет bootstrap из `config.paths.units_dir`, actual unit execution lane и полного ingress context parity

Также важно фиксировать терминологию без разъезда:

- ingress classes: `realtime | recovery | scheduled | manual`
- update traits: `text | callback | photo | voice | job | reply`
- command: отдельный index (`warn | mute | ban | del | undo | msg`), а не transport/update bucket

Именно это сейчас отличает код от новой целевой модели.

## Что в коде уже совпадает с новой моделью

Это важно не потерять:

- event-first flow реально есть
- typed parser chain реально есть
- built-in moderation execution lane уже есть
- audit и replay contracts уже несут business meaning
- dry-run уже first-class
- tg idempotency и storage replay уже разведены

То есть база под indexed router уже собрана, просто верхний routing слой еще не построен.

## Практический вывод для следующего этапа

Первый смысловой шаг теперь должен звучать так:

не “собрать какой-то router вообще”,
а “добавить runtime-level classification и dispatch indexes поверх уже существующего moderation core”.

Порядок:

1. `Runtime`
2. classification model
3. bucket indexes
4. ingress pipeline
5. unit bootstrap из `config.paths.units_dir`
6. unit envelopes и actual unit execution

## Итоговая формулировка

На текущем состоянии проекта:

- docs теперь считают целевой моделью indexed/bucketed router
- код уже реализует runtime path с router/ingress для built-in moderation
- основной разрыв находится в unit bootstrap/execution и в live ingress context coverage
