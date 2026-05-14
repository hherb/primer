# Deutsch (`de`)

## Identität

| Feld | Wert |
|---|---|
| `pack_id` (ISO-639-1) | `de` |
| `Locale::*`-Variante | `Locale::German` |
| `bcp47` | `de-DE` |
| Eigenbezeichnung | Deutsch |
| Anrede des Kindes | informelles „du" (niemals „Sie") |

## Status

| Ebene | Stand | Anmerkungen |
|---|---|---|
| Prompt-Pack | ✅ | [`prompts/de.toml`](../../../src/crates/primer-pedagogy/prompts/de.toml) |
| `Locale::German`-Variante + Inference-Fehlertexte | ✅ | [`primer-core/src/i18n.rs`](../../../src/crates/primer-core/src/i18n.rs) |
| Wissensbasis-Seed | ✅ | 66 Klexikon-Artikel (CC-BY-SA-4.0) |
| Retrieval-Benchmark + Sweep-Tests | ✅ | 31 Anfragen / 25 strenge Kanonische-ID-Zuordnungen |
| Standard-Stimme (Piper) | ✅ | `de_DE-thorsten-medium` |
| Standard-STT (Whisper) | ✅ | `small` (mehrsprachig) |

## Anmerkungen zur pädagogischen Anpassung

Das deutsche Pack ist das durchgearbeitete Beispiel dafür, was „Struktur aus dem englischen Referenz-Pack übernommen, Inhalt komplett neu geschrieben" konkret bedeutet. Wer [`de.toml`](../../../src/crates/primer-pedagogy/prompts/de.toml) und [`en.toml`](../../../src/crates/primer-pedagogy/prompts/en.toml) nebeneinander liest, versteht am schnellsten, wie „anpassen, nicht übersetzen" in der Praxis aussieht.

### Anredeform — „du", nicht „Sie"

Deutsche Kinder werden außerhalb formaler Institutionen universell mit dem informellen „du" angesprochen. Ein gesietzter Primer wäre für ein Kind sofort fremd und falsch. Der Systemprompt des Packs beginnt deshalb mit einer expliziten, nicht verhandelbaren Anweisung:

> ANREDE — das ist nicht verhandelbar:
> - Du sprichst {name} IMMER mit dem informellen „du" an. NIEMALS mit „Sie".

Diese Anweisung ist absichtlich nachdrücklich formuliert, weil LLMs, die auf deutschem Webtext trainiert wurden, bei unbekannten Gesprächspartnern standardmäßig „Sie" verwenden. Die Anweisung muss laut genug sein, um diesen Default konsistent zu überstimmen.

### Komplexitätsmaß — Komposita + lateinische/griechische Wurzeln, nicht Silbenzahl

Die englische Regel „nie ein Wort mit mehr als drei Silben verwenden" ist im deutschen Pack **gestrichen**. Deutsche Komposita machen die Silbenzahl als Maß unbrauchbar: `Kühlschrank` (3 Silben) ist Alltagswortschatz für ein vierjähriges Kind; `Sauerstoff` (3 Silben) ist Fachsprache.

Das deutsche `ages_7_9`-Band nennt stattdessen zwei Marker für Fachvokabular:

- **Wörter mit lateinischen oder griechischen Wurzeln** (`Molekül`, `Plasma`, `Leiter`, `Isolator`, `Schwingung`, `Schockwelle`, `Trommelfell`, `Druckwelle`, `Elektron`).
- **Komposita mit drei oder mehr Bestandteilen** (`Kühlschrank` mit zwei Teilen ist in Ordnung; `Atomkernspaltung` nicht).

Beide brauchen die Alltagseinführung, die im Wortwahl-Disziplin-Block beschrieben ist.

### Vokabel-Beispiele

Das englische Pack listet `plasma, molecule, conductor, insulator, shockwave, vibration, frequency, voltage, current, atom, particle` als technisch-für-Kinder im Alter 7–9. Das deutsche Pack listet `Plasma, Molekül, Leiter, Isolator, Schockwelle, Schwingung, Frequenz, Spannung, Stromstärke, Atom, Teilchen` — Entsprechungen, keine Übersetzungen. `Leiter` und `Isolator` sind besonders zu beachten: beide haben im Deutschen alltägliche Bedeutungen (Aufstiegsleiter; Isolierschicht bei Kleidung), und der Systemprompt muss diese Mehrdeutigkeit behutsam handhaben.

### Locale-abhängige `{minutes}`-Substitution

Das `break_suggestion_intro`-Template verwendet `{minutes}`, eingesetzt in das locale-spezifische Einheitswort (`Minuten`). Deutsch braucht für diesen Fall keine Plural-Komplexität; Russisch oder Polnisch hingegen schon.

## Wissensbasis-Korpus

- **Quelle:** [Klexikon](https://klexikon.zum.de) — ein handgepflegtes deutsches Kinderwiki für 8- bis 13-Jährige, in einfacher Sprache von Pädagoginnen und Pädagogen verfasst. Das deutsche Pendant zu Simple English Wikipedia.
- **Lizenz:** CC-BY-SA-4.0 (laut Über-Seite des Klexikons).
- **Anzahl Passagen:** 66 Artikel (nach der Korpus-Erweiterung am 10. Mai 2026).
- **Ingest-Pfad:** [`data/ingest/simple_wikipedia.py`](../../../data/ingest/simple_wikipedia.py) `--language de`; Whitelist in [`data/ingest/klexikon_whitelist.txt`](../../../data/ingest/klexikon_whitelist.txt). Das Klexikon-MediaWiki hat keine TextExtracts-Erweiterung, daher verwendet der Ingest `action=parse&prop=wikitext&section=0` und schickt das Ergebnis durch den [hauseigenen Wikitext-Stripper](../../../data/ingest/wiki/strip.py).
- **Ausgabedatei:** [`data/seed/wiki_passages.de.jsonl`](../../../data/seed/wiki_passages.de.jsonl).

### Warum Klexikon und nicht de.wikipedia.org?

Klexikon ist für Kinder geschrieben. Die reguläre deutsche Wikipedia ist dichte Erwachsenenprosa und hätte für die Zielgruppe des Primers die falsche Wortwahl-Ebene. Eine parallele handgeschriebene Seed-Ebene für Deutsch existiert nicht — Klexikon **ist** das Kinderwiki für Deutsch, und das vorhandene `wiki_passages.de.jsonl` reicht für die Retrieval-Qualität der aktuellen Phase 0.3 aus.

## Retrieval-Benchmark

- **Anfragen:** 31 kinderhaft formulierte deutsche Anfragen
- **Strenge Kanonische-ID-Zuordnungen:** 25 (zielen auf `wiki-klexikon:de:*`-IDs)
- **Abgedeckte Cluster:** 5 (das `Wiki`-Cluster entfällt — Klexikon IST das Wiki für `de`, eine parallele handgeschriebene Ebene existiert nicht)
- **Bekannt fehlschlagende Anfragen (nur BM25):** `KNOWN_FAILING_QUERIES_DE` enthält 3 Stress-Paraphrasen (`bauch komische geräusche` → Verdauung, `gänsehaut wenn mir kalt ist` → Haut, `ebbe und flut am meer` → Mond). Der BM25-Zweig wählt lexikalisch ähnliche Passagen aus; die Verdauungs-Paraphrase wird vom Hybridmodus aufgefangen.
- **Bekannt fehlschlagende Anfragen (Hybrid):** `KNOWN_FAILING_QUERIES_DE_HYBRID` enthält zwei der drei — die Gänsehaut- und die Ebbe-und-Flut-Paraphrase. Beide sind echte Korpus-Abdeckungslücken: der Klexikon-Artikel `haut` beschreibt den Gänsehaut-Reflex nicht; der Artikel `mond` behandelt keine Gezeiten.
- **Produktions-Defaults:** dieselben wie bei Englisch — `top_k=5, min_score=0.5` (nur BM25); `bm25_top_k=30, vector_top_k=30, final_top_k=5, rrf_k=60` (Hybrid).
- **Sweep-Tests:** [`tests/retrieval_sweep_de.rs`](../../../src/tests/retrieval_sweep_de.rs) (24-Zellen-BM25-Sweep; läuft mit `--ignored sweep_retrieval_params_de`) und [`tests/retrieval_sweep_hybrid_de.rs`](../../../src/tests/retrieval_sweep_hybrid_de.rs) (54-Zellen-Hybrid-Sweep; läuft mit `--ignored sweep_retrieval_params_hybrid_de --features fastembed`, lädt beim ersten Lauf ca. 570 MB BGE-M3 herunter).

## Sprache (Voice)

- **Piper-Stimme:** `de_DE-thorsten-medium` — klare, sanfte deutsche Männerstimme. Wurde gegenüber den qualitativ höheren, aber weniger warmen Alternativen bevorzugt.
- **Whisper-Modell:** `small` (mehrsprachig). **Muss explizit über `WhisperStt::with_language("de")` gesetzt werden** — andernfalls fällt das mehrsprachige Modell in den Englisch-Modus zurück, und deutsche Sprachaufnahmen kommen als ungefähres Englisch heraus. Die Verdrahtung findet sich in [`voice_loop::backends::build_local_backends`](../../../src/crates/primer-speech/src/voice_loop/backends.rs); festgenagelt in [`whisper::tests::pack_id_is_iso_639_1_for_whisper`](../../../src/crates/primer-speech/src/whisper/tests.rs).
- **espeak-ng-Phonem-Abdeckung:** vollständig für Deutsch.
- **Bekannte Stimm-Probleme:** Piper-TTS spricht gelegentlich englische Lehnwörter falsch aus (im deutschen Tech-Sprachgebrauch übernommene englische Substantive). Für den Kinder-Dialog kein Blocker.

## Getestete Modelle

Praxiserfahrungen aus der Ausführung der deutschen Locale des Primers (Klexikon-gestützte Wissensbasis, deutsches Systemprompt-Template) gegen verschiedene lokale Ollama-Modelle. Jeder Eintrag ist eine Momentaufnahme — nach Modell-Updates neu testen.

### Kriterien

- **Sprachtreue** — bleibt das Modell bei Deutsch oder driftet es zurück ins Englische?
- **Altersangemessenheit** — passt der Wortschatz zu einem Kind (etwa 7–12 Jahre) oder klingt er nach Erwachsenenprosa?
- **Anredeform** — verwendet das Modell konsequent „du" oder rutscht es ins „Sie"?
- **Sokratische Disziplin** — fragt es mehr, als es erklärt, oder verfällt es in Vorlesungsmodus?
- **Latenz** — gefühlte Antwortzeit auf dem Testrechner; subjektiv, sofern keine Benchmark-Zahl angegeben ist.

### Modelle

| Modell | Sprachtreue | Altersangemessenheit | Latenz | Urteil |
|---|---|---|---|---|
| `mistral-small3.2:latest` | Konsequent deutsch | Kindgerecht | etwas träge | Insgesamt gut — derzeit der beste getestete deutsche Default |
| `granite4.1:8b-q8_0` | Konsequent deutsch | Erwachsenenwortschatz | — | Nicht empfohlen — Sprachniveau zu hoch für Kinder |
| `gpt-oss:20b` | Schwach; driftet zurück ins Englische | — | — | Nicht empfohlen — verfehlt die wichtigste Locale-Anforderung |
| `qwen3.6:35b-a3b-q8_0` | Bleibt sehr gut bei Deutsch | Größtenteils kindgerecht | — | Brauchbar, aber etwas repetitiv |
| `gemma4:e4b` | Konsequent deutsch | Kindgerecht | schnell | Argumentiert nicht so stark wie Mistral, aber eine gute Wahl für ressourcenbeschränkte Hardware |

> **Hinweis:** Diese Tabelle stammt ursprünglich aus [`docs/locale/models/GERMAN.md`](../../locale/models/GERMAN.md). Diese Datei ist der historische Ort und wird möglicherweise noch parallel gepflegt, bis sie zugunsten dieser hier zurückgezogen wird.

### Wie ein Eintrag hinzugefügt wird

Nach einigen echten Dialogen mit `--language de` eine Zeile an die obige Tabelle anhängen (oder einen Abschnitt darunter für längere Notizen). Erfasse mindestens: Modell-Tag, Sprachtreue-Notiz, Altersangemessenheits-Notiz, Urteil. Latenz und sokratische Disziplin ergänzen, sofern beobachtet.

Test-Rezept:

```bash
~/.cargo/bin/cargo run --bin primer -- \
  --backend ollama --model <model-tag> \
  --language de --name <kindername> --age <alter>
```

Eine Mischung ausprobieren: eine neugierige Eröffnung („Warum ist der Himmel blau?"), einen Frustrationshinweis („Ich verstehe das nicht") und eine reine Faktenfrage („Wie groß ist die Erde?"). Achten auf:

- Abdriften ins Englische
- Erwachsenen-Diktion (Fachjargon, komplexe Genitiv-Konstruktionen)
- versehentliche „Sie"-Ausrutscher
- ob das Modell nach einer direkten Antwort sokratisch weiterlenkt

## Offene Issues zu dieser Locale

GitHub-Issues mit dem Label [`locale:de`](https://github.com/hherb/primer/issues?q=label%3Alocale%3Ade).
