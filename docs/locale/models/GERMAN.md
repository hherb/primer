# Tested models — German locale (`--language de`)

Hands-on observations from running the Primer's German locale (Klexikon-backed knowledge base, German system-prompt template) against various local Ollama models. Each entry is a snapshot — re-test after model updates.

## Criteria

- **Language adherence** — does the model stay in German, or drift back to English?
- **Age appropriateness** — is the vocabulary suited to a child (roughly ages 7–12), or adult prose?
- **Socratic discipline** — does it ask more than it tells, or fall into lecture mode?
- **Latency** — perceived response time on the test machine; subjective unless a benchmark number is given.

## Models

| Model | Language adherence | Age appropriateness | Latency | Verdict |
|---|---|---|---|---|
| `mistral-small3.2:latest` | Consistent German | Child-appropriate | A bit sluggish | Good overall — current best-tested German default |
| `granite4.1:8b-q8_0` | Consistent German | Adult vocabulary | — | Not recommended — language level too high for children |
| `gpt-oss:20b` | Poor; drifts back to English | — | — | Not recommended — fails the primary locale requirement |
| `qwen3.6:35b-a3b-q8_0` | Sticks very well to German | Mostly child-appropriate | — | Usable, but a bit repetitive |
| `gemma4:e4b` | Consistent German | Child appropriate | fast | Not reasoning as well as Mistral, but good choice for constrained hardware |

## How to add an entry

After running a model through a few real exchanges with `--language de`, append a row to the table above (or a section below for longer notes). Capture at minimum: model tag, language-adherence note, age-appropriateness note, verdict. Add latency and Socratic-discipline notes if you have them.

Test recipe:

```bash
cargo run --bin primer -- \
  --backend ollama --model <model-tag> \
  --language de --name <child-name> --age <age>
```

Try a mix of: a curiosity opener ("Warum ist der Himmel blau?"), a frustration cue ("Ich verstehe das nicht"), and a factual question ("Wie groß ist die Erde?"). Watch for: drift to English, adult diction (Fachjargon, complex Genitive constructions), and whether the model pivots Socratically after a direct answer.
