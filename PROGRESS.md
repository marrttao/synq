# PROGRESS

## Этап 1. Каркас — выполнен

Сделано:
- `SPEC.md` — главный документ (текст задания).
- `Cargo.toml`: ratatui 0.30, crossterm, serde, toml, anyhow, thiserror, tracing (+appender, subscriber); dev: tempfile.
- `src/paths.rs` — определение BASE (PLAYER_HOME → папка exe с `music/` → cwd), создание папок. Тесты.
- `src/config.rs` — `config.toml` с комментариями создаётся при первом запуске; частичный файл дополняется дефолтами; битый файл не перезаписывается, плеер стартует с дефолтами и пишет ошибку в строку статуса. Тесты.
- Логи в `data/logs/synq.log.<дата>`, ничего не пишется в stdout. Паники логируются.
- `src/ui/` — пустой интерфейс: адаптивная раскладка (Library / Lyrics / Visual / нижняя панель), ~30 fps, выход по `q` или `Ctrl+C`, терминал восстанавливается при выходе, ошибке и панике (`ratatui::try_init` ставит panic hook). Цвета из `[theme]`.
- `.gitignore`, `music/.gitkeep`, `gifs/.gitkeep`.
- 13 unit-тестов, `cargo clippy --all-targets -- -D warnings` без предупреждений.

Не сделано / отложено сознательно:
- Остальные крейты стека (rodio, lofty, rusqlite, notify, reqwest, image, ratatui-image, tachyonfx, rubato, symphonia, whisper-rs) добавляются по мере этапов, где они нужны, чтобы не тянуть долгую сборку раньше времени. Фича `whisper` будет объявлена на этапе 6 (whisper-rs требует cmake и libclang — это пойдёт в README).
- `Config::save` и `Paths::db` пока не используются (помечены `allow(dead_code)`).

Известные проблемы:
- Проверено только на Windows. Linux — этап 9.
- `Config::save` не сохраняет комментарии ручного файла.
- Модуль `paths.rs` добавлен сверх списка модулей из SPEC (BASE нужен отдельным тестируемым кодом).
