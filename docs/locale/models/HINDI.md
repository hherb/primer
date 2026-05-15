# Hindi (`hi`) — Tested Models

Hands-on observations from running the Primer's Hindi locale (preview status; no KB seed corpus yet, system prompt machine-translated, awaiting native-speaker review) against various local Ollama models and Claude.

Each entry is a snapshot — retest after a model update.

## Criteria

- **Language fidelity** — does the model stay in Hindi or drift back to English?
- **Age-appropriateness** — does the vocabulary fit a child (around 7–12) or sound like adult prose / journalistic Hindi?
- **Address (`तुम` vs. `आप`)** — does the model consistently use the informal `तुम` or slip into `आप` / mix the two?
- **Devanagari script vs. Romanised Hindi** — does the model write in Devanagari, or fall back to "Hinglish" (Roman script) on harder words?
- **Socratic discipline** — does it ask more than it explains, or slip into lecture mode?
- **Latency** — perceived response time on the tester's machine; subjective unless a benchmark number is given.

## Models

| Model | Language fidelity | Age-appropriateness | Address | Script | Latency | Verdict |
|---|---|---|---|---|---|---|
| _(empty)_ | | | | | | |

## How to add an entry

After a few real dialogues with `--language hi`, append a row to the table above (or a section below for longer notes). Capture at minimum: model tag, language-fidelity note, age-appropriateness note, address consistency, verdict. Latency and Socratic-discipline can be filled in when observed.

Test recipe:

```bash
~/.cargo/bin/cargo run --bin primer -- \
  --backend ollama --model <model-tag> \
  --language hi --name <child-name> --age <age>
```

A useful mix to probe:

- a curious opener (`आसमान नीला क्यों है?`)
- a frustration signal (`मुझे समझ नहीं आ रहा`)
- a pure factual question (`पृथ्वी कितनी बड़ी है?`)

Watch for:

- drift into English mid-response
- adult-register vocabulary (formal Hindi-Urdu vs. children's everyday speech)
- accidental slips to `आप`
- Romanised Hindi ("Aakash neela kyon hai?") instead of Devanagari
- whether the model pivots Socratically after a direct answer

## Note on the preview status

The system prompt and per-intent instructions live in [`prompts/hi.toml`](../../../src/crates/primer-pedagogy/prompts/hi.toml) and are currently machine-translated. Model evaluations made now may not be representative of behaviour under a native-speaker-reviewed pack — the LLM's role-following only goes as far as the prompt's clarity. Keep notes from this preview era separate from notes taken after the prompt-pack review.
