# Benchmark Fixtures

This directory contains audio clips used by `tests/benchmark.rs` to measure
Character Error Rate (CER) for Japanese speech recognition.

## Datasets

### `tatoeba/` — Original clean subset (9 clips)

Hand-picked native Japanese speech recordings from [Tatoeba](https://tatoeba.org).
These are clear, full sentences with minimal background noise.

- **CER**: ~2.04%
- **Source**: Tatoeba Japanese audio (CC-BY 2.0 FR)

### `tatoeba_extended/` — Expanded colloquial subset (25 clips)

Additional short, colloquial phrases from Tatoeba. These are more challenging:
many are 2–6 words, include exclamations, and use kanji variants that the model
sometimes renders in kana (e.g., "誠に" → "まことに").

- **CER**: ~21%
- **Combined CER** (34 clips): ~12.67%
- **Source**: Tatoeba Japanese audio (CC-BY 2.0 FR)

## Adding More Clips

The benchmark automatically picks up any subdirectory with a `meta.txt` file.

Format of `meta.txt` (tab-separated):

```
filename.mp3\ttranscript text here
another.wav\tanother transcript
```

The benchmark tries the exact filename first; if not found, it falls back to
replacing `.mp3` with `.wav` for backward compatibility.

### Suggested sources for expansion

- **JSUT corpus** (~7,600 utterances, single speaker, 10h):  
  http://ss-takashi.sakura.ne.jp/corpus/jsut_ver1.1.zip
- **Common Voice Japanese** (~6,000+ validated clips, many speakers):  
  https://commonvoice.mozilla.org/ja/datasets
- **JVS corpus** (100 speakers, 30h, multi-style):  
  https://sites.google.com/site/shinnosuketakamichi/research-topics/jvs_corpus

## License

Audio files retain the license of their original source (Tatoeba: CC-BY 2.0 FR).
They are included here solely for reproducible benchmarking.
