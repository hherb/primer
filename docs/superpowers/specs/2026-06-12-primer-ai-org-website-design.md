# primer-ai.org — Project Website Design

**Date:** 2026-06-12
**Status:** Approved
**Domain:** primer-ai.org (registered with Cloudflare; hosted on Cloudflare Pages free tier)

## Purpose and audience

A public homepage for the Primer project. Primary audiences, in order:

1. **Researchers & educators** — pedagogy-minded readers evaluating the Socratic approach and the learning-science grounding.
2. **Funders / partners** — people assessing the project for grants, hardware partnerships, or collaboration.

Developers and curious parents are secondary; they are served by the GitHub link and the plain-language landing copy respectively. The site's job is credibility: communicate the vision, show the design principles are deliberate and research-grounded, and prove the project is real and working (on-device NPU numbers, working voice loop, multilingual support).

## Site map

Five static pages plus a 404:

| File | Title | Purpose |
|---|---|---|
| `index.html` | The Primer | Story-scroll landing page — full pitch on one page |
| `vision.html` | Vision & pedagogy | Diamond Age inspiration, Socratic method, design principles expanded with research citations |
| `technology.html` | Technology | Local-first architecture, privacy model, backend abstraction, what works today, validated platforms |
| `roadmap.html` | Roadmap & status | Phase plan: 0.1–0.3 complete, 1.x in progress, 2–4 ahead; dated milestones |
| `get-involved.html` | Get involved | Who we want to hear from + contact email + GitHub |
| `404.html` | Not found | Styled 404 (Cloudflare Pages serves it automatically) |

Shared header nav (Vision · Technology · Roadmap · Get involved) and footer (contact email, GitHub link, license note) on every page.

## Landing page structure (story scroll)

A visitor who never clicks deeper still gets the whole story:

1. **Hero** — gold seal emblem, "The Primer", tagline *"A Socratic AI learning companion for children"*, one-line pitch ("It doesn't teach by telling. It teaches by asking."), two CTAs: *Read the vision* → `vision.html`, *View on GitHub* → repo.
2. **What it is** — 2–3 short paragraphs: the Diamond Age hook, what the Primer does in a child's day, what makes it different from ed-tech apps.
3. **Design principles** — card grid of six: asks more than it answers; never maximises engagement; comprehension verified, not assumed; voice-first by pedagogy; runs fully offline; all data stays local.
4. **"It works today" evidence band** — navy background, gold accents. Key proof points: ~9.4 tok/s on-device NPU inference (Snapdragon 8 Elite Gen 5, 2026-06-09); complete offline voice loop (VAD → Whisper → LLM → Piper); English + German production, Hindi preview; open source (AGPL) with working desktop GUI and CLI.
5. **Explore cards** — four doorway cards to the detail pages.
6. **Footer.**

## Detail page content

All copy is derived from `README.md`, `ROADMAP.md`, and `primer_technical_spec.md`, rewritten for a non-developer audience. No crate names or CLI flags on the landing/vision pages; the technology page may go one level deeper but stays prose-first.

- **Vision & pedagogy:** the Young Lady's Illustrated Primer inspiration; the Socratic method as implemented (direct answer then pivot for factual questions; transfer questions, application challenges, contradiction probing for comprehension); the anti-engagement stance (detects frustration/disengagement, offers breaks and session close without guilt); the voice-first rationale with the research grounding (Goldin-Meadow 2009 on gesture and learning transfer; conversation demands active construction); spaced repetition woven into conversation rather than drilled.
- **Technology:** local-first / airgap-capable design; privacy model (learner model never leaves the device; cloud inference per-request only, opt-in); the hardware-abstraction idea in plain terms (same pedagogical engine, swappable inference backends — cloud, local llama.cpp, phone NPU); what works today (streaming chat, long-term memory, engagement/concept/comprehension classifiers, learner model, hybrid retrieval over a curated children's corpus, voice loop, desktop GUI); validated platforms (macOS, Linux, Android/Termux, Hexagon NPU); multilingual prompt packs.
- **Roadmap & status:** phase table — Phase 0 (cloud-backed proof of pedagogy) complete; Phase 1 (local inference: llama.cpp landed, Qualcomm NPU validated on-device, RKNN ahead) in progress; Phase 2 (speech hardening) partially landed ahead of schedule; later phases (dedicated hardware, anonymised corpus contribution) as vision. Dated milestones: Termux validation 2026-05-26, NPU validation 2026-06-09.
- **Get involved:** explicit asks — educators/researchers to evaluate pedagogy or pilot; funders/partners for hardware and study support; native speakers (esp. Hindi) for prompt-pack review; developers via GitHub. Contact: `contact@primer-ai.org`.

## Visual system ("Light Academic")

- **Palette:** parchment white `#fdfcf8` base; ink `#1a2032` text; navy `#1a2b5c` primary accent; muted gold `#8a6d1f` for labels/eyebrows (bright gold `#d4af37` reserved for use on navy bands); hairline borders `#e8e4d8`.
- **Type:** serif system stack (Georgia, 'Times New Roman', serif) for headings and body; small uppercase letter-spaced labels for eyebrows. No webfonts.
- **Zero external requests:** no external fonts, scripts, analytics, or trackers — the site practices the project's own privacy principles. Static assets only.
- **Artwork:** existing project assets copied into `website/assets/` — seal emblem (`curious_childs_primer_icon.png`) in the hero and as favicon source, banner (`curious_childs_primer_banner_medium.png`) as the Open Graph image, illustration available for the vision page.
- **Responsive:** single shared `style.css`; mobile nav via CSS-only pattern (or minimal inline JS if needed); card grids collapse to single column.
- **Meta:** per-page titles/descriptions, Open Graph + Twitter card tags, canonical URLs on `https://primer-ai.org/`.

## Repository layout and deployment

```
website/
├── index.html
├── vision.html
├── technology.html
├── roadmap.html
├── get-involved.html
├── 404.html
├── style.css
├── assets/            # images copied from repo assets/
└── README.md          # deployment + email-routing instructions
```

- **Source of truth:** `website/` directory at the top level of the public `hherb/primer` repo.
- **Deployment:** Cloudflare Pages, git integration. Build command: *none*. Root directory: `website`. Auto-deploys on push to `main`; PR branches get preview URLs for free.
- **Custom domains:** `primer-ai.org` and `www.primer-ai.org` attached to the Pages project (Cloudflare manages DNS since the domain is registered there).
- **Manual one-time steps (owner, via Cloudflare dashboard; documented step-by-step in `website/README.md`):**
  1. Create the Pages project and connect it to `hherb/primer` with root directory `website`.
  2. Add the two custom domains.
  3. Enable Email Routing and create `contact@primer-ai.org` → forward to personal inbox.

## Error handling

- `404.html` in the output directory — Cloudflare Pages serves it for unknown paths automatically.
- No forms, no JS-dependent functionality, so no client-side error states. The `mailto:` contact link works everywhere.

## Testing / verification

- Local check: serve `website/` with any static server (`python3 -m http.server`) and click through all nav paths, verify responsive layout at phone width.
- HTML validity: pages kept simple/semantic; spot-check with a validator.
- Post-deploy: verify both domains resolve with HTTPS, OG preview renders (e.g. via a link-preview checker), 404 page serves on a bogus path, and the contact address forwards.

## Out of scope (YAGNI)

- No blog, no CMS, no static site generator, no npm toolchain.
- No analytics (deliberate, on principle).
- No contact form / Worker — email link only.
- No screenshots/demo videos in v1 (can be added later as plain `<img>`/`<video>`).
- No localised site versions (English only for v1, even though the product is multilingual).
