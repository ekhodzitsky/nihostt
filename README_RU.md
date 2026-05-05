# nihostt

<p align="center">
  <h1 align="center">nihostt</h1>
  <p align="center"><strong>Локальное распознавание японской речи с CER 2.04%</strong></p>
  <p align="center">Сервер STT на базе ReazonSpeech-k2-v2 — без облака, без API-ключей, полная приватность</p>
  <p align="center">
    <a href="https://github.com/ekhodzitsky/nihostt/actions"><img src="https://github.com/ekhodzitsky/nihostt/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
    <a href="CHANGELOG.md"><img src="https://img.shields.io/badge/changelog-Keep%20a%20Changelog-orange" alt="Changelog"></a>
  </p>
  <p align="center"><a href="README.md">English</a> | <b>Русский</b></p>
</p>

---

**nihostt** превращает любой компьютер в сервер распознавания японской речи в реальном времени. Один бинарник, одна команда, точность на уровне лучших облачных решений — всё работает локально.

```bash
git clone https://github.com/ekhodzitsky/nihostt.git
cd nihostt && cargo build --release
./target/release/nihostt download && ./target/release/nihostt serve
# WebSocket: ws://127.0.0.1:9876/v1/ws
# REST API:  http://127.0.0.1:9876/v1/transcribe
```

## Почему nihostt?

| | nihostt | Google Cloud Speech | Azure Speech | Amazon Transcribe |
|---|---|---|---|---|
| **Приватность** | ✅ 100% на устройстве | ❌ Облако | ❌ Облако | ❌ Облако |
| **Оффлайн** | ✅ Работает без интернета | ❌ Нет | ❌ Нет | ❌ Нет |
| **Задержка** | ✅ ~200 мс (локально) | ~500–2000 мс | ~500–2000 мс | ~1000–3000 мс |
| **Стоимость** | ✅ Бесплатно навсегда | $0.024/мин | $1.0/час | $0.024/мин |
| **CER (японский)** | **2.04%** | ~5–8% | ~4–7% | ~5–9% |

*Бенчмарк: 9 клипов носителей языка из Tatoeba, character error rate. См. [`tests/benchmark.rs`](tests/benchmark.rs).*

## Возможности

- 🎙️ **Стриминг в реальном времени** — WebSocket с частичными и финальными результатами
- 📁 **Загрузка файлов** — REST для пакетной транскрипции (WAV, MP3, M4A, FLAC, OGG)
- 📡 **SSE-стриминг** — Server-Sent Events для прогрессивной транскрипции файлов
- 🧠 **SOTA точность** — ReazonSpeech-k2-v2 (Zipformer RNN-T, 159M параметров)
- ⚡ **INT8 квантизация** — ~155 МБ модель, ~350 МБ RAM на мобильных
- 🔒 **Приватность по умолчанию** — только loopback, никакой телеметрии
- 📱 **Android FFI** — сборка `libnihostt.so` для мобильного STT
- 🗣️ **Диаризация спикеров** — опциональное определение спикеров

## Быстрый старт

```bash
# 1. Сборка из исходников
git clone https://github.com/ekhodzitsky/nihostt.git
cd nihostt
cargo build --release

# 2. Скачивание модели (~155 МБ INT8, один раз)
./target/release/nihostt download

# 3. Запуск сервера
./target/release/nihostt serve

# 4. Транскрипция файла
./target/release/nihostt transcribe recording.wav
```

### Пример стриминга через WebSocket

```javascript
const ws = new WebSocket('ws://127.0.0.1:9876/v1/ws');

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  if (msg.type === 'partial') console.log('partial:', msg.text);
  if (msg.type === 'final')   console.log('final:  ', msg.text);
};

ws.onopen = () => {
  navigator.mediaDevices.getUserMedia({ audio: true })
    .then(stream => {
      const recorder = new MediaRecorder(stream);
      recorder.ondataavailable = e => {
        e.data.arrayBuffer().then(buf => ws.send(buf));
      };
      recorder.start(500);
    });
};
```

См. [`examples/`](examples/) для клиентов на Python, Kotlin, Go, Bun и JavaScript.

## Архитектура

```
┌─────────────┐     WebSocket     ┌─────────────────────────────┐
│   Browser   │◄─────────────────►│  axum server (Rust)         │
│   / Mobile  │     REST/SSE      │  ├── VAD (Silero)            │
└─────────────┘                   │  ├── Session pool (4× ONNX)  │
                                  │  └── Streaming pipeline      │
                                  └─────────────────────────────┘
                                           │
                                           ▼
                                  ┌─────────────────────────────┐
                                  │  ONNX Runtime               │
                                  │  ├── Encoder (INT8, ~155MB) │
                                  │  ├── Decoder (~4MB)         │
                                  │  └── Joiner (~2.6MB)        │
                                  └─────────────────────────────┘
```

## Бенчмарки

Запущено локально на Apple M1 Pro:

```bash
cargo test --test benchmark -- --ignored
```

| Датасет | Тип | CER | Примечания |
|---|---|---|---|
| Tatoeba JA (9 клипов) | Реальная речь носителей | **2.04%** | См. [`tests/fixtures/tatoeba/`](tests/fixtures/tatoeba/) |
| Синтетический TTS | `say -v Kyoko` | 24.19% | Акустический мисматч |

## Установка

### macOS (Homebrew)

> Доступно после первого релиза. Формула готова в [`Formula/nihostt.rb`](Formula/nihostt.rb).

```bash
brew tap ekhodzitsky/nihostt
brew install nihostt
```

### Из исходников

```bash
git clone https://github.com/ekhodzitsky/nihostt.git
cd nihostt
cargo build --release
./target/release/nihostt serve
```

### Docker

```bash
# CPU (любая платформа)
docker build -t nihostt .
docker run -p 9876:9876 nihostt

# CUDA (Linux, требуется NVIDIA Container Toolkit)
docker build -f Dockerfile.cuda -t nihostt-cuda .
docker run --gpus all -p 9876:9876 nihostt-cuda

# Встроенная модель (~350 МБ)
docker build --build-arg NIHOSTT_BAKE_MODEL=1 -t nihostt:baked .
```

## API

| Метод | Эндпоинт | Описание |
|---|---|---|
| `GET` | `/health` | Проверка работоспособности |
| `POST` | `/v1/transcribe` | Загрузка аудио, JSON-результат |
| `POST` | `/v1/transcribe/stream` | Загрузка аудио, SSE-стрим |
| `WS` | `/v1/ws` | Реальное время: partial/final |

См. [`docs/openapi.yaml`](docs/openapi.yaml) для REST/SSE и [`docs/asyncapi.yaml`](docs/asyncapi.yaml) для WebSocket протокола. Полный справочник по CLI — в [`docs/cli.md`](docs/cli.md).

## Android / Мобильные

Сборка `libnihostt.so` для Android:

```bash
cargo ndk -t arm64-v8a -o ./android/app/src/main/jniLibs \
  build --release --features ffi
```

См. [`ANDROID.md`](ANDROID.md) для полного руководства по интеграции.

## Модель

| Файл | Размер | Описание |
|---|---|---|
| `encoder-epoch-99-avg-1.onnx` | ~155 МБ (INT8) | Квантованный Zipformer encoder |
| `decoder-epoch-99-avg-1.onnx` | ~4.4 МБ | LSTM decoder |
| `joiner-epoch-99-avg-1.onnx` | ~2.6 МБ | RNN-T joiner |
| `tokens.txt` | ~46 КБ | BPE словарь (5224 токена) |

Авто-скачивание с [HuggingFace](https://huggingface.co/reazon-research/reazonspeech-k2-v2) при первом запуске.

## Участие в проекте

Приветствуем! См. [`CONTRIBUTING.md`](CONTRIBUTING.md).

```bash
cargo test && cargo clippy && cargo deny check
```

## Лицензия

MIT — см. [`LICENSE`](LICENSE).

---

⭐ **Поставьте звезду, если проект полезен!** Это помогает другим находить приватное распознавание японской речи.
