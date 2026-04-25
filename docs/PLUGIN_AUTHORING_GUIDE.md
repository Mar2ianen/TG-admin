# Plugin Authoring Guide

Этот документ описывает, как писать plugin/unit manifests и handlers так, чтобы они:

- совпадали с текущими контрактами кода
- не ломали будущий indexed/bucketed router
- не тащили обратно плоский scan по всем units

Важно: на `2026-04-22` полный unit-driven runtime path еще не собран.  
То есть plugins как отдельные route handlers уже нужно проектировать сейчас, но live execution для них еще не подключен end-to-end.

## Что считается plugin в этом проекте

В коде plugin сейчас соответствует `unit`:

- TOML manifest с секциями `Unit`, `Trigger`, `Service`, `Capabilities`, `Runtime`
- handler script, на который указывает `Service.ExecStart`
- capability envelope, который ограничивает, что unit вообще может делать

Исходный контракт manifest живет в [src/unit.rs](/home/arch/Документы/Teloxide/src/unit.rs:30).

## Главный принцип

Плагин должен быть узким и bucket-oriented.

Правильно:

- один plugin на одну routing-задачу
- один plugin на один command family
- один plugin на один media/update intent
- отдельные plugins для `voice`, `photo`, `private`, `callback`, `job`

Неправильно:

- один giant plugin “на все сообщения”
- один plugin с большим `if/else` внутри под text, voice, photo, callback и scheduled сразу
- перенос routing-логики из ядра в скрипт

Router должен выбирать plugin по bucket/index, а не plugin должен сам угадывать, зачем его вызвали.

## Что уже валидно в manifest сейчас

Текущая схема `Trigger` поддерживает только три формы:

- `command`
- `regex`
- `event_type`

То есть прямо сейчас manifest **еще не умеет** декларативно сказать:

- `voice bucket`
- `photo bucket`
- `private chat bucket`
- `linked channel comment bucket`
- `author_kind bucket`

Эти buckets уже есть как routing model в [src/router.rs](/home/arch/Документы/Teloxide/src/router.rs:73), но schema manifest пока не догнала router полностью.

Поэтому важно разделять:

- **текущий валидный manifest contract**
- **целевую bucket authoring model**

## Минимальная структура plugin

Рекомендуемый layout:

```text
units/
  moderation.warn.unit.toml
  media.voice.transcribe.unit.toml
  private.autoreply.unit.toml

scripts/
  moderation/warn.rhai
  media/voice_transcribe.rhai
  private/autoreply.rhai
```

Имя unit должно быть стабильным и описывать bucket + intent.

Хорошо:

- `moderation.warn.unit`
- `media.voice.transcribe.unit`
- `private.welcome.unit`
- `callback.approve.unit`
- `job.cleanup.expired_mutes.unit`

Плохо:

- `plugin1`
- `main_handler`
- `all_in_one.unit`

## Базовый manifest

```toml
[Unit]
Name = "moderation.warn.unit"
Description = "Built-in compatible warn flow"
Enabled = true
Tags = ["moderation", "command"]
Owner = "admin"
Version = "1.0.0"

[Trigger]
Type = "command"
Commands = ["warn"]

[Service]
ExecStart = "scripts/moderation/warn.rhai"
EntryPoint = "main"
TimeoutSec = 3
Restart = "no"
RestartSec = 1
MaxRetries = 0

[Capabilities]
Allow = [
  "tg.read_basic",
  "db.user.write",
  "tg.write_message",
  "audit.read"
]

[Runtime]
DryRunSupported = true
IdempotentByDefault = false
AllowInRecovery = false
AllowManualInvoke = true
```

## Какие triggers использовать сейчас

### `command`

Используй для явных slash-команд:

- `/warn`
- `/mute`
- `/ban`
- `/del`
- `/undo`
- `/msg`
- будущие plugin-команды вроде `/transcribe`

Пример:

```toml
[Trigger]
Type = "command"
Commands = ["transcribe"]
```

### `regex`

Используй только для text-driven plugins, где match действительно текстовый.

Подходит для:

- поиск ссылок
- поиск запрещенных слов
- auto-tagging по тексту

Не подходит для:

- voice/photo/video handlers
- callback handlers
- private/group routing

Пример:

```toml
[Trigger]
Type = "regex"
Pattern = "(?i)\\bfree\\s+money\\b"
```

### `event_type`

Сейчас валидные значения:

- `message`
- `callback_query`
- `job`

Пример:

```toml
[Trigger]
Type = "event_type"
Events = ["message"]
```

Это ближайшая текущая основа для некомандных plugins, но она пока слишком широкая для точного bucket routing.

## Как писать plugins под будущие buckets уже сейчас

Хотя schema еще не умеет явно выражать `voice/photo/private/...`, plugins уже стоит раскладывать так, как будто bucket manifests скоро появятся.

Рекомендуемая модель:

- один TOML на один bucket intent
- один script entrypoint на один handler
- название unit и путь script должны явно отражать bucket

Примеры целевых intent buckets:

- `message.text.*`
- `message.photo.*`
- `message.voice.*`
- `message.video.*`
- `message.document.*`
- `message.sticker.*`
- `message.contact.*`
- `message.location.*`
- `message.poll.*`
- `callback.*`
- `private.*`
- `group.*`
- `supergroup.*`
- `channel_post.*`
- `linked_channel_comment.*`
- `job.*`

Пока manifest schema не расширена, такие plugins нужно держать как отдельные units по смыслу, а не склеивать в один файл.

То есть уже сейчас пиши:

- `media.voice.transcribe.unit.toml`
- `media.photo.ocr.unit.toml`
- `private.support.autoreply.unit.toml`
- `callback.moderation.approve.unit.toml`

А не:

- `media_handlers.unit.toml`
- `misc.unit.toml`

## Bucket naming convention

Чтобы потом безболезненно перейти на явные bucket manifests, придерживайся такого порядка в имени:

`<scope>.<content_or_source>.<intent>.unit`

Примеры:

- `message.voice.transcribe.unit`
- `message.photo.ocr.unit`
- `private.welcome.reply.unit`
- `callback.report.resolve.unit`
- `job.cleanup.expired_mutes.unit`
- `channel_comment.autoreply.unit`

Если plugin командный:

`command.<name>.unit`

Примеры:

- `command.transcribe.unit`
- `command.stats.unit`

## Capabilities: брать минимум

Не выдавай plugin лишние capability.

Часто нужны:

- `tg.read_basic`
- `tg.write_message`
- `tg.moderate.delete`
- `tg.moderate.restrict`
- `tg.moderate.ban`
- `db.user.read`
- `db.user.write`
- `msg.history.read`
- `job.schedule`
- `audit.read`
- `audit.compensate`
- `sys.http.fetch`
- `ml.stt`
- `ml.embed_text`

Полный текущий allow-list живет в [src/unit.rs](/home/arch/Документы/Teloxide/src/unit.rs:474).

Правило:

- если plugin только читает событие и отвечает текстом, не давай `db.user.write` или `tg.moderate.*`
- если plugin не планирует jobs, не давай `job.schedule`
- если plugin не делает ML, не давай `ml.*`

## Runtime flags

Смысл текущих флагов:

- `DryRunSupported = true`
  - plugin можно безопасно прогонять в dry-run
- `IdempotentByDefault = true`
  - plugin не должен плодить side effects при повторном запуске
- `AllowInRecovery = true`
  - plugin безопасен в replay/recovery path
- `AllowManualInvoke = true`
  - plugin можно дергать вручную

Консервативный дефолт:

- `DryRunSupported = true`
- `IdempotentByDefault = false`
- `AllowInRecovery = false`
- `AllowManualInvoke = true`

Если не уверен, оставляй recovery выключенным.

## Какой plugin писать для разных кейсов

### Text/command plugin

Используй:

- `Trigger.Type = "command"` для slash-команд
- `Trigger.Type = "regex"` для content-match по тексту

### Callback plugin

Используй:

- `Trigger.Type = "event_type"`
- `Events = ["callback_query"]`

И держи unit узким по смыслу:

- `callback.report.resolve.unit`
- `callback.moderation.undo.unit`

### Scheduled/job plugin

Используй:

- `Trigger.Type = "event_type"`
- `Events = ["job"]`

Примеры:

- `job.cleanup.expired_mutes.unit`
- `job.daily.summary.unit`

### Voice/photo/other media plugin

На уровне naming/layout уже делай отдельный plugin под каждый media kind.

Примеры:

- `message.voice.transcribe.unit`
- `message.photo.ocr.unit`
- `message.document.extract.unit`

Но важно: текущая schema manifest еще не умеет точно привязать unit к `voice/photo/document` bucket напрямую. Это целевая модель, а не уже собранный runtime contract.

Поэтому такие plugins сейчас нужно писать как подготовленные units, а точную bucket binding логику мы добавим при расширении unit routing schema.

### Private-chat plugin

Так же:

- выделяй private-flow в отдельный unit
- не смешивай его с group handlers

Пример:

- `private.welcome.reply.unit`
- `private.support.router.unit`

Но прямой manifest-level trigger “only private chat” пока еще не реализован.

### Linked channel comments

Если нужен plugin на комментарии под постами канала:

- делай отдельный unit
- не смешивай его с обычным group text flow
- в имени отражай именно этот source

Пример:

- `channel_comment.autoreply.unit`
- `channel_comment.filter.unit`

Сейчас router уже различает этот кейс как отдельный route signal приблизительно, но manifest-level binding под него еще не оформлен.

## Что script не должен делать

Script не должен:

- повторно определять тип апдейта через giant `if/else`
- сам решать, `voice` это или `photo`, если router уже знает bucket
- объединять unrelated flows в один handler
- требовать capability “про запас”

Если внутри handler появляются ветки вроде:

```text
if private ...
else if callback ...
else if voice ...
else if photo ...
else if command ...
```

значит plugin спроектирован неправильно.

## Практическое правило на завтра

Если завтра будешь накидывать plugins уже сейчас, безопасный порядок такой:

1. Делай отдельный unit на каждый intent.
2. Давай unit понятное bucket-oriented имя.
3. Разделяй scripts по типу контента и по source.
4. Используй только реально поддержанный `Trigger`.
5. Не изобретай в TOML новые поля, которых schema пока не знает.
6. Для `voice/photo/private/channel_comment` считай manifest пока подготовительным, а не финально подключенным runtime-контрактом.

## Чего пока нельзя обещать plugin author

На `2026-04-22` пока нельзя обещать:

- что любой media bucket plugin уже автоматически подцепится runtime
- что `private/group/channel_comment` можно выразить в manifest без расширения schema
- что live Telegram ingress уже подаст все events в unit-driven lane

То есть authoring guideline уже можно соблюдать сейчас, но полная execution story для всех buckets еще впереди.

## Короткий шаблон

```toml
[Unit]
Name = "message.voice.transcribe.unit"
Description = "Voice transcription handler"
Enabled = true
Tags = ["voice", "ml", "message"]
Owner = "admin"
Version = "0.1.0"

[Trigger]
Type = "event_type"
Events = ["message"]

[Service]
ExecStart = "scripts/message/voice_transcribe.rhai"
EntryPoint = "main"
TimeoutSec = 3
Restart = "no"
RestartSec = 1
MaxRetries = 0

[Capabilities]
Allow = ["tg.read_basic", "tg.write_message", "ml.stt"]

[Runtime]
DryRunSupported = true
IdempotentByDefault = false
AllowInRecovery = false
AllowManualInvoke = false
```

Этот шаблон **корректен как unit manifest сегодня**, но его `voice`-семантика пока выражена именованием и layout, а не отдельным declarative trigger field.

## Итог

Правильный plugin для этого проекта:

- маленький
- узкий
- bucket-oriented
- capability-minimal
- без giant branching внутри script
- с именем, из которого сразу видно `scope + source + intent`

Если потом расширим schema под явные media/private/channel buckets, такие plugins можно будет почти безболезненно перевести на точный declarative routing.
