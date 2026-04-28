# Deployment Runbook

## Inputs

- Set `TMO_BOT_TOKEN` for the live token, or place `telegram.bot_token` in `config.toml`.
- Keep the token out of the repo. Use `telegram.polling = true` for live Telegram ingress.
- Set `telegram.admin_user_ids` to the Telegram user IDs that are allowed to run moderation commands.
- Set `telegram.primary_chat_ids` to the chats that must be bootstrapped on startup.

## Startup

```sh
export TMO_BOT_TOKEN='123456:REDACTED'
cargo run --release
```

- The runtime resolves the token from `telegram.bot_token` or `TMO_BOT_TOKEN`.
- If polling is disabled or the token is missing, the bot stays in local/noop mode.
- During startup, the runtime calls `getMe`, then checks each `primary_chat_id` with `GetChatAdministrators`.
- Startup fails if the bot is not an admin in any configured primary chat.

## Bootstrap Notes

- `admin_user_ids` is the known-admin allowlist for moderation and ingress context.
- `primary_chat_ids` are operational bootstrap targets, not a separate runtime role.
- Add the bot to every primary chat first, then grant admin rights before the first live start.

## Smoke Test

1. Start the service and watch the startup logs for token resolution and primary-chat bootstrap.
2. From a user in `telegram.admin_user_ids`, send a safe command such as `/msg smoke`.
3. In a test chat, verify a moderation command like `/warn` produces the expected log entry and Telegram response.

## Logs

- For systemd, follow the unit logs with `journalctl -u <service-name> -f`.
- If file logging is enabled, check `paths.log_dir` from `config.toml`.
- Logs may include chat IDs, user IDs, message text, and bot/admin state.

## Rollback

1. Stop the service.
2. Restore the previous binary or release artifact.
3. Restore the previous `config.toml` if the config changed.
4. Restart the service.

## Retention And Privacy

- Keep logs and database backups for the minimum time needed for operations.
- Treat message text, chat IDs, user IDs, and file IDs as sensitive operational data.
- Never commit or paste live bot tokens, full logs, or raw database dumps.
