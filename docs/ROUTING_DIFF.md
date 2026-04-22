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

- runtime-level event classification
- bucket/index build
- `dispatch set` router
- общего ingress pipeline

То есть:

- routing fragments есть
- working moderation route есть
- indexed runtime router еще отсутствует

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

Но все еще не делает:

- Telegram ingest loop
- unit bootstrap
- event classification
- bucket index build
- execution dispatch

Вывод:

lifecycle shell есть, отдельного `Runtime` и router пока нет.

## 2. Реальный working route в коде

Сейчас реально работает такой локальный built-in path:

`EventContext -> EventCommandDispatcher -> ParsedCommandLine -> ExpandedCommandLine -> ModerationEngine -> execute_*`

Именно он сейчас выполняет built-in moderation semantics.

То есть `ModerationEngine` сейчас является рабочим local executor/route для built-in moderation, но не top-level runtime router.

Вывод:

command routing внутри moderation есть, runtime routing tree с buckets нет.

## 3. Telegram layer

Есть:

- typed request/result contract
- dry-run behavior
- idempotency shell

Нет:

- live transport по умолчанию
- production ingress loop

Вывод:

Telegram execution contract есть, transport/ingress path нет.

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
| Runtime composition root | Должен собирать router, ingress и executors | `Application` пока только lifecycle shell + storage bootstrap | Partial |
| Event classification | Должен быть отдельный runtime шаг | В runtime отсутствует | Missing |
| Bucket/index build | Должен строиться на startup/reload | Нет | Missing |
| Dispatch set routing | Должен выбирать релевантные bucket groups | Нет | Missing |
| Built-in moderation path | Должен существовать как одна из execution lanes | Есть локально внутри `ModerationEngine` | Present |
| Command parser chain | Должен сохраняться как typed path | Есть | Present |
| Journaling | Должен быть ingress concern | Есть storage API, но не встроен в live path | Missing |
| Dedupe/replay | Должен стоять до destructive effects | В moderation уже безопаснее, но ingress-level path еще не собран | Partial |
| Unit bootstrap | Должен грузиться на startup | В runtime не встроен | Missing |
| Event -> unit envelope | Должен быть явный binding | Для realtime path отсутствует | Missing |
| Built-in vs scriptable boundary | Должна быть явной execution lane boundary | Пока фактически built-in only | Partial |
| Live Telegram transport | Должен быть production transport | По умолчанию `noop` | Missing |

## Главное расхождение

Главный gap уже не формулируется как “нет decision tree вообще”.

Точнее формулировка такая:

- есть локальный moderation decision path
- нет runtime-level indexed router, который делает classification и dispatch set

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
5. built-in execution через router
6. unit envelopes

## Итоговая формулировка

На текущем состоянии проекта:

- docs теперь считают целевой моделью indexed/bucketed router
- код пока реализует только локальный built-in moderation route
- основной разрыв находится в classification, bucket indexes, ingress и unit-aware execution envelopes
