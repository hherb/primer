//! The text REPL: open or resume the session, then read lines until a
//! quit word or EOF. Moved verbatim out of `async_main`.

use std::io::{self, BufRead, Write};
use std::path::Path;

use primer_core::error::PrimerError;
use primer_core::i18n::{Locale, render_inference_error};
use primer_core::storage::SessionStore;
use primer_pedagogy::DialogueManager;
use primer_storage::SqliteSessionStore;

use crate::Cli;

/// Run the REPL on `dm` until the child quits or stdin closes.
pub(crate) async fn run_repl(
    cli: &Cli,
    dm: &mut DialogueManager,
    session_store: &SqliteSessionStore,
    session_path: &Path,
    cli_locale: Locale,
) -> anyhow::Result<()> {
    // ─── REPL ────────────────────────────────────────────────────────

    if let Some(resume_id) = cli.resume {
        match session_store.load_session(resume_id).await? {
            None => {
                eprintln!(
                    "Error: no session with id {resume_id} found in {}",
                    session_path.display()
                );
                std::process::exit(1);
            }
            Some(loaded) => {
                let n_turns = loaded.turns.len();
                dm.resume_session(loaded).await?;
                eprintln!("\nResumed session {resume_id} with {n_turns} prior turn(s).\n");
            }
        }
    } else {
        let greeting = dm.open_session().await?;
        println!("\nPrimer: {greeting}\n");
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = line?;
        let input = line.trim();

        // Exit commands.
        if input.is_empty() {
            continue;
        }
        if input.eq_ignore_ascii_case("quit")
            || input.eq_ignore_ascii_case("exit")
            || input.eq_ignore_ascii_case("bye")
        {
            dm.close_session().await;
            println!("\nPrimer: That was a good conversation. Until next time.\n");
            break;
        }

        // Generate the Primer's response, printing tokens as they arrive.
        // The "Primer: " prefix is held back until the first non-empty chunk
        // so that an immediate failure (no API key, ollama down) doesn't leave
        // a dangling "Primer: " above the error message.
        let mut prefix_printed = false;
        let result = dm
            .respond_to_streaming(input, |chunk| {
                if !prefix_printed {
                    print!("\nPrimer: ");
                    prefix_printed = true;
                }
                print!("{chunk}");
                let _ = io::stdout().flush();
            })
            .await;
        match result {
            Ok(_) => {
                if prefix_printed {
                    println!("\n");
                } else {
                    eprintln!("\n(no response generated)\n");
                }
            }
            Err(PrimerError::Inference(inf)) => {
                if prefix_printed {
                    println!();
                }
                tracing::warn!(error = %inf, "inference failed; surfacing user-friendly message");
                eprintln!("{}\n", render_inference_error(&inf, &cli_locale));
            }
            Err(other) => {
                if prefix_printed {
                    println!();
                }
                eprintln!("Error generating response: {other}\n");
            }
        }

        // Print pedagogical debug info when --verbose is set.
        if cli.verbose {
            if let Some(intent) = dm.last_intent() {
                eprintln!(
                    "[intent] {:?} -> {:?}",
                    dm.learner.current_engagement, intent
                );
            }
            if let Some(a) = dm.last_assessment() {
                let r = a.reasoning.as_deref().unwrap_or("");
                eprintln!(
                    "[classifier] {:?} conf={:.2} ({})",
                    a.state,
                    a.confidence,
                    dm.classifier_identifier()
                );
                if !r.is_empty() {
                    eprintln!("             — {r}");
                }
            }
            if let Some(e) = dm.last_extraction() {
                eprintln!(
                    "[extractor] child={:?} primer={:?} ({})",
                    e.child_concepts,
                    e.primer_concepts,
                    dm.extractor_identifier()
                );
            }
            if let Some(c) = dm.last_comprehension() {
                if c.assessments.is_empty() {
                    eprintln!("[comprehension] (none) ({})", dm.comprehension_identifier());
                } else {
                    let pairs: Vec<String> = c
                        .assessments
                        .iter()
                        .map(|a| format!("{}={}({:.2})", a.concept, a.depth, a.confidence))
                        .collect();
                    eprintln!(
                        "[comprehension] {} ({})",
                        pairs.join(" "),
                        dm.comprehension_identifier()
                    );
                }
            }
        }

        print!("{}: ", dm.learner.profile.name);
        stdout.flush()?;
    }

    Ok(())
}
