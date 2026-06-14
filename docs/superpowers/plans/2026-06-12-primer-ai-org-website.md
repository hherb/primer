# primer-ai.org Website Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the static project website for primer-ai.org (landing page + four detail pages) in a new top-level `website/` directory, ready for Cloudflare Pages git-integration deployment.

**Architecture:** Six hand-written static HTML pages sharing one stylesheet, zero build step, zero external requests (no webfonts, no JS frameworks, no analytics). Cloudflare Pages serves the `website/` directory directly; deployment and email-routing setup are one-time manual dashboard steps documented in `website/README.md`.

**Tech Stack:** Plain HTML5 + CSS. Python's `http.server` for local preview only.

**Spec:** `docs/superpowers/specs/2026-06-12-primer-ai-org-website-design.md`

---

## File structure

```
website/
├── index.html          # story-scroll landing page
├── vision.html         # Vision & pedagogy
├── technology.html     # Technology & architecture
├── roadmap.html        # Roadmap & status
├── get-involved.html   # Get involved / contact
├── 404.html            # styled not-found page (Cloudflare Pages picks it up automatically)
├── style.css           # single shared stylesheet (Light Academic system)
├── assets/
│   ├── emblem.png      # copy of assets/curious_childs_primer_icon_medium.png (307×307 seal)
│   ├── banner.png      # copy of assets/curious_childs_primer_banner_medium.png (704×384, OG image)
│   └── illustration.png# copy of assets/primer_illustration.png (vision page)
└── README.md           # deployment + email-routing instructions for the owner
```

Conventions used by every page:

- Internal links use the `.html` form (`vision.html`) so they work under `python3 -m http.server` and `file://`; Cloudflare Pages auto-redirects them to clean URLs in production.
- `<link rel="canonical">` and `og:url` use the clean production form (`https://primer-ai.org/vision`).
- Every page carries the same header/nav and footer markup (repeated verbatim — no templating, by design).
- The current page's nav link gets `aria-current="page"`.
- All copy is plain English for a researcher/educator/funder audience — no crate names or CLI flags except on the technology page, which may name Rust and the model/runtime stack.

---

### Task 1: Scaffold `website/` and copy assets

**Files:**
- Create: `website/assets/` (directory)
- Copy: `assets/curious_childs_primer_icon_medium.png` → `website/assets/emblem.png`
- Copy: `assets/curious_childs_primer_banner_medium.png` → `website/assets/banner.png`
- Copy: `assets/primer_illustration.png` → `website/assets/illustration.png`

- [ ] **Step 1: Create the directory and copy the three images**

Run from the repo root:

```bash
mkdir -p website/assets
cp assets/curious_childs_primer_icon_medium.png website/assets/emblem.png
cp assets/curious_childs_primer_banner_medium.png website/assets/banner.png
cp assets/primer_illustration.png website/assets/illustration.png
```

- [ ] **Step 2: Verify the copies**

Run: `ls -la website/assets/`
Expected: `emblem.png` (~224 KB), `banner.png` (~540 KB), `illustration.png` (~5.9 MB).

- [ ] **Step 3: Commit**

```bash
git add website/assets
git commit -m "feat(website): scaffold website/ with project artwork assets"
```

---

### Task 2: Shared stylesheet `style.css`

**Files:**
- Create: `website/style.css`

- [ ] **Step 1: Write the complete stylesheet**

Create `website/style.css` with exactly this content:

```css
/* primer-ai.org — Light Academic design system.
   Palette and type per docs/superpowers/specs/2026-06-12-primer-ai-org-website-design.md.
   No webfonts, no imports — zero external requests by design. */

:root {
  --parchment: #fdfcf8;
  --card: #ffffff;
  --ink: #1a2032;
  --muted: #555c6e;
  --navy: #1a2b5c;
  --navy-deep: #14224a;
  --gold: #8a6d1f;
  --gold-bright: #d4af37;
  --hairline: #e8e4d8;
}

* { box-sizing: border-box; }

html { scroll-behavior: smooth; }

body {
  margin: 0;
  background: var(--parchment);
  color: var(--ink);
  font-family: Georgia, 'Times New Roman', serif;
  font-size: 17px;
  line-height: 1.65;
}

img { max-width: 100%; height: auto; }

a { color: var(--navy); }
a:hover { color: var(--gold); }

.container { max-width: 1080px; margin: 0 auto; padding: 0 24px; }
.prose { max-width: 70ch; }
.prose p { margin: 0 0 1em; }

/* ---------- header ---------- */

.site-header { border-bottom: 1px solid var(--hairline); }

.header-inner {
  max-width: 1080px;
  margin: 0 auto;
  padding: 14px 24px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  flex-wrap: wrap;
}

.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  text-decoration: none;
  color: var(--ink);
  font-size: 1.05rem;
}

.brand-mark { width: 34px; height: 34px; border-radius: 50%; }

.site-nav { display: flex; gap: 22px; flex-wrap: wrap; }

.site-nav a {
  text-decoration: none;
  color: var(--muted);
  font-size: 0.95rem;
  letter-spacing: 0.3px;
  padding-bottom: 2px;
  border-bottom: 2px solid transparent;
}

.site-nav a:hover,
.site-nav a[aria-current="page"] {
  color: var(--navy);
  border-bottom-color: var(--gold-bright);
}

/* ---------- typography helpers ---------- */

.eyebrow {
  color: var(--gold);
  font-size: 0.78rem;
  letter-spacing: 2.5px;
  text-transform: uppercase;
  margin: 0 0 10px;
}

h1 {
  font-size: clamp(2rem, 5vw, 2.9rem);
  line-height: 1.15;
  font-weight: normal;
  margin: 0 0 14px;
}

h2 { font-size: 1.6rem; font-weight: normal; margin: 0 0 16px; }
h3 { font-size: 1.15rem; font-weight: normal; margin: 0 0 8px; }

.lede { font-size: 1.15rem; color: var(--muted); max-width: 62ch; }

.citation {
  font-size: 0.88rem;
  color: var(--muted);
  border-left: 3px solid var(--gold-bright);
  padding-left: 14px;
  margin: 1.2em 0;
}

/* ---------- hero (landing) ---------- */

.hero { text-align: center; padding: 72px 24px 64px; }

.hero .emblem {
  width: 110px;
  height: 110px;
  border-radius: 50%;
  box-shadow: 0 0 0 4px var(--gold-bright);
  margin-bottom: 26px;
}

.hero h1 { margin-bottom: 8px; }
.hero .lede { margin: 18px auto 30px; }

.cta-row { display: flex; gap: 14px; justify-content: center; flex-wrap: wrap; }

.btn {
  display: inline-block;
  background: var(--navy);
  color: #fff;
  text-decoration: none;
  font-size: 0.8rem;
  letter-spacing: 1.5px;
  text-transform: uppercase;
  padding: 13px 28px;
  border-radius: 2px;
}

.btn:hover { background: var(--navy-deep); color: #fff; }

.btn-outline { background: transparent; color: var(--navy); border: 1px solid var(--navy); }
.btn-outline:hover { background: var(--navy); color: #fff; }

/* ---------- sections ---------- */

.section { padding: 56px 0; }
.section + .section { border-top: 1px solid var(--hairline); }

/* page hero for subpages */
.page-hero { padding: 56px 0 16px; }

/* ---------- card grids ---------- */

.grid {
  display: grid;
  gap: 18px;
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
}

.card {
  background: var(--card);
  border: 1px solid var(--hairline);
  border-top: 3px solid var(--navy);
  padding: 22px;
}

.card h3 { color: var(--navy); }
.card p { margin: 0; font-size: 0.95rem; color: var(--muted); }

/* ---------- evidence band ---------- */

.band { background: var(--navy); color: #f0e6d2; }
.band .eyebrow { color: var(--gold-bright); }
.band h2 { color: #fff; }
.band a { color: var(--gold-bright); }
.band .section-pad { padding: 56px 0; }

.stats {
  display: grid;
  gap: 28px;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  margin-top: 28px;
}

.stat-num { font-size: 1.9rem; color: var(--gold-bright); line-height: 1.2; }
.stat-label { font-size: 0.92rem; color: #c7d0e8; margin-top: 6px; }

/* ---------- explore cards ---------- */

.explore-card {
  display: block;
  background: var(--card);
  border: 1px solid var(--hairline);
  padding: 22px;
  text-decoration: none;
  transition: border-color 0.15s ease;
}

.explore-card:hover { border-color: var(--gold-bright); }
.explore-card h3 { color: var(--navy); }
.explore-card p { color: var(--muted); font-size: 0.95rem; margin: 0 0 10px; }
.explore-card .go { color: var(--gold); font-size: 0.85rem; letter-spacing: 1px; text-transform: uppercase; }

/* ---------- roadmap ---------- */

.badge {
  display: inline-block;
  font-size: 0.7rem;
  letter-spacing: 1.5px;
  text-transform: uppercase;
  padding: 3px 10px;
  border-radius: 2px;
  vertical-align: middle;
  margin-left: 10px;
}

.badge-done { background: #e4efe4; color: #2e5d2e; border: 1px solid #bcd6bc; }
.badge-progress { background: #fdf3dc; color: #8a6d1f; border: 1px solid #ecd9a4; }
.badge-ahead { background: #eef0f5; color: #555c6e; border: 1px solid #d8dce6; }

.phase {
  background: var(--card);
  border: 1px solid var(--hairline);
  padding: 24px;
  margin-bottom: 18px;
}

.phase h3 { color: var(--navy); }
.phase ul { margin: 12px 0 0; padding-left: 20px; color: var(--muted); font-size: 0.97rem; }
.phase li { margin-bottom: 4px; }

.milestone {
  display: flex;
  gap: 18px;
  padding: 12px 0;
  border-bottom: 1px dotted var(--hairline);
}

.milestone time { color: var(--gold); white-space: nowrap; font-size: 0.92rem; }
.milestone p { margin: 0; }

/* ---------- vision page figure ---------- */

.figure { margin: 2em 0; text-align: center; }
.figure img { max-width: 420px; width: 100%; border: 1px solid var(--hairline); border-radius: 8px; }
.figure figcaption { font-size: 0.85rem; color: var(--muted); margin-top: 10px; }

/* ---------- footer ---------- */

.site-footer {
  border-top: 1px solid var(--hairline);
  margin-top: 56px;
  padding: 36px 24px 44px;
  text-align: center;
}

.site-footer p { margin: 0 0 8px; }
.fineprint { font-size: 0.85rem; color: var(--muted); }

/* ---------- responsive ---------- */

@media (max-width: 720px) {
  .hero { padding: 48px 16px 40px; }
  .section { padding: 40px 0; }
  .page-hero { padding: 40px 0 8px; }
}
```

- [ ] **Step 2: Commit**

```bash
git add website/style.css
git commit -m "feat(website): Light Academic shared stylesheet"
```

---

### Task 3: Landing page `index.html`

**Files:**
- Create: `website/index.html`

- [ ] **Step 1: Write the complete page**

Create `website/index.html` with exactly this content:

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>The Primer — A Socratic AI learning companion for children</title>
<meta name="description" content="The Primer is an open-source, voice-first AI learning companion for children. It teaches by asking, never maximises engagement, runs fully offline, and keeps all learner data on the device.">
<link rel="canonical" href="https://primer-ai.org/">
<link rel="icon" type="image/png" href="assets/emblem.png">
<meta property="og:title" content="The Primer — A Socratic AI learning companion for children">
<meta property="og:description" content="It doesn't teach by telling. It teaches by asking. Open source, voice-first, fully offline, and never engineered for engagement.">
<meta property="og:image" content="https://primer-ai.org/assets/banner.png">
<meta property="og:url" content="https://primer-ai.org/">
<meta property="og:type" content="website">
<meta name="twitter:card" content="summary_large_image">
<link rel="stylesheet" href="style.css">
</head>
<body>

<header class="site-header">
  <div class="header-inner">
    <a class="brand" href="index.html"><img class="brand-mark" src="assets/emblem.png" alt=""><span>The Primer</span></a>
    <nav class="site-nav">
      <a href="vision.html">Vision</a>
      <a href="technology.html">Technology</a>
      <a href="roadmap.html">Roadmap</a>
      <a href="get-involved.html">Get involved</a>
    </nav>
  </div>
</header>

<main>

<section class="hero">
  <img class="emblem" src="assets/emblem.png" alt="The Primer seal — a child reading inside an illuminated book">
  <p class="eyebrow">A Socratic AI learning companion for children</p>
  <h1>The Primer</h1>
  <p class="lede">It doesn't teach by telling. It teaches by asking. When a child wonders why the sky is blue, the Primer doesn't recite Rayleigh scattering — it asks what colour the sky turns at sunset, and walks the child toward discovering the answer themselves.</p>
  <div class="cta-row">
    <a class="btn" href="vision.html">Read the vision</a>
    <a class="btn btn-outline" href="https://github.com/hherb/primer">View on GitHub</a>
  </div>
</section>

<section class="section">
  <div class="container">
    <p class="eyebrow">What it is</p>
    <h2>A patient conversation, not another app</h2>
    <div class="prose">
      <p>The Primer is an open-source learning companion inspired by the Young Lady's Illustrated Primer in Neal Stephenson's <em>The Diamond Age</em> — a book that converses with one child, adapts to her, and teaches her to think. We are building the nearest thing today's technology honestly allows: a voice-first companion that holds genuine Socratic conversations, remembers what a child understands, and runs entirely on hardware in the child's hands.</p>
      <p>It is not an app competing for a child's attention. There is no feed, no streaks, no points, no notifications. There is a patient conversational partner that asks good questions, listens to the answers, verifies understanding rather than assuming it, and knows when to suggest a break.</p>
    </div>
  </div>
</section>

<section class="section">
  <div class="container">
    <p class="eyebrow">Design principles</p>
    <h2>Six commitments that don't change</h2>
    <div class="grid">
      <div class="card">
        <h3>Asks more than it answers</h3>
        <p>Pure factual questions get a direct answer — then a pivot: "Now that you know the Moon is 384,000&nbsp;km away, how long would a car take to drive there?"</p>
      </div>
      <div class="card">
        <h3>Never maximises engagement</h3>
        <p>The Primer detects frustration and disengagement and responds with scaffolding, a topic change, or "that's enough for today" — never guilt, never a hook.</p>
      </div>
      <div class="card">
        <h3>Comprehension is verified, not assumed</h3>
        <p>Understanding is probed through transfer questions, application challenges, and gentle contradictions — not inferred from a confident-sounding reply.</p>
      </div>
      <div class="card">
        <h3>Voice-first by pedagogy</h3>
        <p>Conversation cannot be skimmed; it demands active thinking. A voice-only companion frees a child's hands and body to gesture, move, and manipulate the world while reasoning.</p>
      </div>
      <div class="card">
        <h3>Runs fully offline</h3>
        <p>Designed to work airgapped on local hardware. Cloud inference is an option, never a dependency — learning shouldn't require connectivity or a subscription.</p>
      </div>
      <div class="card">
        <h3>All data stays local</h3>
        <p>The learner model — what a child knows, how deeply, what holds their attention — never leaves the device without explicit parental consent.</p>
      </div>
    </div>
  </div>
</section>

<section class="band">
  <div class="container section-pad">
    <p class="eyebrow">It works today</p>
    <h2>Not a concept — running software</h2>
    <div class="stats">
      <div class="stat">
        <div class="stat-num">~9.4 tok/s</div>
        <div class="stat-label">Language model running on a phone's neural processor — validated on a Snapdragon&nbsp;8 Elite handset, June&nbsp;2026</div>
      </div>
      <div class="stat">
        <div class="stat-num">Voice loop</div>
        <div class="stat-label">Listen → think → speak, entirely on-device: voice detection, transcription, generation, and speech synthesis with no cloud</div>
      </div>
      <div class="stat">
        <div class="stat-num">2 + 1 languages</div>
        <div class="stat-label">English and German production-ready; Hindi in preview awaiting native-speaker review</div>
      </div>
      <div class="stat">
        <div class="stat-num">100% open</div>
        <div class="stat-label">AGPL-licensed source with a working desktop app and command-line interface — <a href="https://github.com/hherb/primer">inspect everything</a></div>
      </div>
    </div>
  </div>
</section>

<section class="section">
  <div class="container">
    <p class="eyebrow">Explore</p>
    <h2>Go deeper</h2>
    <div class="grid">
      <a class="explore-card" href="vision.html">
        <h3>Vision &amp; pedagogy</h3>
        <p>The Diamond Age inspiration, the Socratic method as implemented, and the learning science behind voice-first design.</p>
        <span class="go">Read more →</span>
      </a>
      <a class="explore-card" href="technology.html">
        <h3>Technology</h3>
        <p>Local-first architecture, the privacy model, and what already runs — from laptops to a phone's neural processor.</p>
        <span class="go">Read more →</span>
      </a>
      <a class="explore-card" href="roadmap.html">
        <h3>Roadmap &amp; status</h3>
        <p>What's complete, what's in progress, and the path to a dedicated child-friendly device.</p>
        <span class="go">Read more →</span>
      </a>
      <a class="explore-card" href="get-involved.html">
        <h3>Get involved</h3>
        <p>For educators, researchers, funders, hardware partners, translators — and anyone who wants to help.</p>
        <span class="go">Read more →</span>
      </a>
    </div>
  </div>
</section>

</main>

<footer class="site-footer">
  <p><a href="mailto:contact@primer-ai.org">contact@primer-ai.org</a> · <a href="https://github.com/hherb/primer">GitHub</a></p>
  <p class="fineprint">© 2026 The Primer project · Code licensed <a href="https://www.gnu.org/licenses/agpl-3.0.html">AGPL-3.0</a> · This site sets no cookies and loads nothing from third parties.</p>
</footer>

</body>
</html>
```

- [ ] **Step 2: Preview locally**

Run: `python3 -m http.server 8901 -d website` (in background), then `curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8901/index.html`
Expected: `200`. Open http://localhost:8901/ in a browser if available: hero emblem renders as a circular medallion, six principle cards, navy evidence band, four explore cards.

- [ ] **Step 3: Commit**

```bash
git add website/index.html
git commit -m "feat(website): story-scroll landing page"
```

---

### Task 4: `vision.html` — Vision & pedagogy

**Files:**
- Create: `website/vision.html`

- [ ] **Step 1: Write the complete page**

Create `website/vision.html` with exactly this content:

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Vision &amp; pedagogy — The Primer</title>
<meta name="description" content="Why the Primer teaches by asking: the Diamond Age inspiration, the Socratic method as implemented, the anti-engagement stance, and the learning science behind voice-first design.">
<link rel="canonical" href="https://primer-ai.org/vision">
<link rel="icon" type="image/png" href="assets/emblem.png">
<meta property="og:title" content="Vision &amp; pedagogy — The Primer">
<meta property="og:description" content="Why the Primer teaches by asking: the Socratic method as implemented, the anti-engagement stance, and the learning science behind voice-first design.">
<meta property="og:image" content="https://primer-ai.org/assets/banner.png">
<meta property="og:url" content="https://primer-ai.org/vision">
<meta property="og:type" content="article">
<meta name="twitter:card" content="summary_large_image">
<link rel="stylesheet" href="style.css">
</head>
<body>

<header class="site-header">
  <div class="header-inner">
    <a class="brand" href="index.html"><img class="brand-mark" src="assets/emblem.png" alt=""><span>The Primer</span></a>
    <nav class="site-nav">
      <a href="vision.html" aria-current="page">Vision</a>
      <a href="technology.html">Technology</a>
      <a href="roadmap.html">Roadmap</a>
      <a href="get-involved.html">Get involved</a>
    </nav>
  </div>
</header>

<main class="container">

<div class="page-hero">
  <p class="eyebrow">Vision &amp; pedagogy</p>
  <h1>Teaching by asking</h1>
  <p class="lede">The Primer is built on a simple conviction: children learn most deeply when they construct understanding themselves, guided by good questions — and that a learning companion should serve the child, never the metrics.</p>
</div>

<section class="section">
  <h2>The inspiration</h2>
  <div class="prose">
    <p>In Neal Stephenson's <em>The Diamond Age</em>, the Young Lady's Illustrated Primer is an interactive book that bonds with one girl and raises her: it converses, tells stories that adapt to her life, and teaches her to think rather than to recite. The fictional Primer needed a hidden human actor behind it. Modern language models make the conversational core honestly buildable for the first time — and small enough to run on hardware a family can own outright.</p>
    <p>We are not building a tutor app with a mascot. We are building the closest real-world counterpart to that book: one child, one companion, a long conversation that spans years.</p>
  </div>
  <figure class="figure">
    <img src="assets/illustration.png" alt="Illustration of an ornate storybook glowing above a tablet device">
    <figcaption>The Primer: a storybook's patience on a device a family can own.</figcaption>
  </figure>
</section>

<section class="section">
  <h2>The Socratic method, implemented</h2>
  <div class="prose">
    <p>Every response the Primer gives is chosen by a pedagogical engine that decides — before generating a single word — what the moment calls for: a guiding question, scaffolding, encouragement, a comprehension probe, or a direct answer.</p>
    <p>Pure factual questions get a direct answer first, because stonewalling a curious child is not pedagogy. But the answer is always followed by a pivot back into inquiry: <em>"The Moon is 384,000 kilometres away. How long do you think a car would take to drive there?"</em> When a child struggles, the Primer scaffolds — breaking the problem into smaller steps rather than revealing the solution. When a child parrots a phrase confidently, the Primer notices, and asks the kind of question a phrase can't answer.</p>
  </div>
</section>

<section class="section">
  <h2>What the Primer refuses to do</h2>
  <div class="prose">
    <p>Most educational software is engineered around engagement: streaks, points, badges, notifications, bright reward animations. These mechanisms work — that is the problem. They train children to seek the reward, not the understanding, and they teach products to compete for attention against the child's own curiosity.</p>
    <p>The Primer has none of them, permanently and by design. It monitors the conversation for frustration and disengagement, and when it detects them it offers scaffolding, suggests a different topic, proposes a break, or simply says "that's enough for today" — without guilt. After half an hour it gently suggests stretching legs. It never blocks a willing child, and it never hooks a tired one.</p>
  </div>
</section>

<section class="section">
  <h2>Comprehension is verified, not assumed</h2>
  <div class="prose">
    <p>A confident-sounding answer is not understanding. The Primer probes depth the way a good teacher does: transfer questions ("Can you explain it to someone who's never heard of it?"), application challenges ("What would happen if gravity were twice as strong?"), and contradiction probing ("Someone told me plants eat soil — what would you say to them?").</p>
    <p>What the child demonstrates is recorded in a longitudinal learner model — every concept encountered, at what depth it's held, and how it develops over weeks and months. Concepts resurface naturally in conversation at expanding intervals, the way a thoughtful adult circles back to last week's topic. There is no drilling and no quizzing; if a word the child learned last month fits today's conversation, the Primer weaves it in and listens to what comes back.</p>
  </div>
</section>

<section class="section">
  <h2>Why voice-first</h2>
  <div class="prose">
    <p>Voice is the Primer's primary interface as a pedagogical choice, not a hardware constraint. Conversational speech demands active construction — you cannot skim a conversation the way you skim text — and that effortful processing is exactly what drives deep learning. A voice-only companion also frees the child's body: hands manipulate objects, arms gesture, feet wander, while the mind reasons.</p>
  </div>
  <p class="citation">Children who gesture while explaining a concept are significantly more likely to transfer that learning to novel problems (Goldin-Meadow, 2009). A device that pins a child's attention to a screen forfeits that — and displaces the parent-child interaction that remains the most powerful learning environment available.</p>
  <div class="prose">
    <p>A screen is available for text, diagrams, and code when a child is older — but it is never required, and for children under roughly eight it is actively undesirable. The Primer should feel like a conversation with a thoughtful adult, not like an app.</p>
  </div>
</section>

<section class="section">
  <h2>A companion, not a product</h2>
  <div class="prose">
    <p>One child, one device. The learner model — the most intimate educational record imaginable — lives on the device and nowhere else, and leaves only with explicit parental consent. The Primer is designed to work with zero connectivity, because a child's access to learning should not depend on a subscription, a server, or a company's continued existence.</p>
  </div>
</section>

</main>

<footer class="site-footer">
  <p><a href="mailto:contact@primer-ai.org">contact@primer-ai.org</a> · <a href="https://github.com/hherb/primer">GitHub</a></p>
  <p class="fineprint">© 2026 The Primer project · Code licensed <a href="https://www.gnu.org/licenses/agpl-3.0.html">AGPL-3.0</a> · This site sets no cookies and loads nothing from third parties.</p>
</footer>

</body>
</html>
```

- [ ] **Step 2: Preview locally**

Run: `curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8901/vision.html` (server from Task 3 still running, else restart it)
Expected: `200`. Visually: page hero, five prose sections, the illustration figure, the gold-bordered citation block.

- [ ] **Step 3: Commit**

```bash
git add website/vision.html
git commit -m "feat(website): vision & pedagogy page"
```

---

### Task 5: `technology.html` — Technology

**Files:**
- Create: `website/technology.html`

- [ ] **Step 1: Write the complete page**

Create `website/technology.html` with exactly this content:

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Technology — The Primer</title>
<meta name="description" content="The Primer's local-first architecture: private by design, swappable inference backends from cloud to a phone's neural processor, and a working voice loop — all open source.">
<link rel="canonical" href="https://primer-ai.org/technology">
<link rel="icon" type="image/png" href="assets/emblem.png">
<meta property="og:title" content="Technology — The Primer">
<meta property="og:description" content="Private by architecture: fully offline operation, swappable inference backends from cloud to a phone's neural processor, and a working voice loop.">
<meta property="og:image" content="https://primer-ai.org/assets/banner.png">
<meta property="og:url" content="https://primer-ai.org/technology">
<meta property="og:type" content="article">
<meta name="twitter:card" content="summary_large_image">
<link rel="stylesheet" href="style.css">
</head>
<body>

<header class="site-header">
  <div class="header-inner">
    <a class="brand" href="index.html"><img class="brand-mark" src="assets/emblem.png" alt=""><span>The Primer</span></a>
    <nav class="site-nav">
      <a href="vision.html">Vision</a>
      <a href="technology.html" aria-current="page">Technology</a>
      <a href="roadmap.html">Roadmap</a>
      <a href="get-involved.html">Get involved</a>
    </nav>
  </div>
</header>

<main class="container">

<div class="page-hero">
  <p class="eyebrow">Technology</p>
  <h1>Private by architecture</h1>
  <p class="lede">The Primer's privacy promises aren't policy — they're structure. The system is designed so that learner data physically cannot leave the device, and so that the entire experience works with the network cable cut.</p>
</div>

<section class="section">
  <h2>Local-first, cloud-optional</h2>
  <div class="prose">
    <p>The Primer runs airgapped: language model, speech recognition, speech synthesis, knowledge base, and the child's learning record all live and execute on the device. Families who choose cloud inference for better conversation quality send individual conversation turns per request — nothing is stored server-side, and the learner model never travels at all.</p>
    <p>This is the inverse of the usual ed-tech architecture, where the child's data lives on the company's servers and the product stops working when the subscription does.</p>
  </div>
</section>

<section class="section">
  <h2>One engine, many brains</h2>
  <div class="prose">
    <p>The pedagogical engine — the part that decides whether this moment calls for a guiding question, scaffolding, or a comprehension probe — is completely decoupled from the language model behind it. Swapping the model is a configuration choice, not a rewrite. The same engine runs against:</p>
  </div>
  <div class="grid" style="margin-top: 24px;">
    <div class="card">
      <h3>Cloud models</h3>
      <p>Anthropic's Claude or any OpenAI-compatible provider, for the highest conversation quality where connectivity and trust allow.</p>
    </div>
    <div class="card">
      <h3>Local models</h3>
      <p>Open-weight models running in-process on a laptop or desktop — CPU or GPU — with no network at all.</p>
    </div>
    <div class="card">
      <h3>Phone neural processors</h3>
      <p>A 4-billion-parameter model on a Snapdragon phone's Hexagon NPU: ~9.4 tokens/s, first token in ~190&nbsp;ms, validated on hardware in June&nbsp;2026.</p>
    </div>
    <div class="card">
      <h3>Hybrid routing</h3>
      <p>An optional per-turn router keeps routine conversation local and escalates only complex turns to a cloud model — opt-in, never default.</p>
    </div>
  </div>
</section>

<section class="section">
  <h2>What works today</h2>
  <div class="prose">
    <p>This is running software, demonstrable end-to-end on a developer laptop and (text mode) on an Android phone:</p>
    <ul>
      <li><strong>Streaming Socratic conversation</strong> with session persistence, resume, and long-term memory that spans hours of dialogue without losing the thread.</li>
      <li><strong>A live learner model</strong> — an engagement classifier, a concept extractor, and a comprehension assessor run quietly behind every exchange, recording what the child encountered and how deeply they understood it.</li>
      <li><strong>Spaced-repetition vocabulary</strong> woven passively into conversation at expanding intervals — no drilling, no quizzes.</li>
      <li><strong>A curated children's knowledge base</strong> — hand-drafted passages plus licensed children's-encyclopedia layers in English and German, searched with hybrid lexical + semantic retrieval, with full source attribution.</li>
      <li><strong>A complete offline voice loop</strong> — voice activity detection, on-device transcription, generation, and natural speech synthesis, with strict turn-taking: the Primer never speaks over the child, and never lets itself be trained to interrupt.</li>
      <li><strong>A desktop app and CLI</strong> — including a developer-facing view of every pedagogical decision the engine makes, per turn.</li>
      <li><strong>Multilingual prompt packs</strong> — English and German production-ready, Hindi in preview; the architecture keeps every user-facing string out of the code so new languages are data, not development.</li>
      <li><strong>Safety plumbing</strong> — chain-of-thought from reasoning models is stripped before it can reach a child, and inference failures degrade to friendly, age-appropriate messages.</li>
    </ul>
  </div>
</section>

<section class="section">
  <h2>Validated platforms</h2>
  <div class="prose">
    <ul>
      <li><strong>macOS and Linux</strong> — full experience: desktop app, CLI, and voice loop (with native Apple speech engines on macOS).</li>
      <li><strong>Android (Snapdragon 8 Elite)</strong> — text conversation validated on-device May&nbsp;2026; the neural-processor inference pipeline validated June&nbsp;2026 at ~9.4 tokens/s, ~190&nbsp;ms to first token, 57&nbsp;°C peak — comfortably inside thermal limits.</li>
    </ul>
    <p>The path to a dedicated, child-friendly device — the Primer as an object a child holds, not an app on a parent's laptop — runs through this phone-class silicon. See the <a href="roadmap.html">roadmap</a>.</p>
  </div>
</section>

<section class="section">
  <h2>Built to be inspected</h2>
  <div class="prose">
    <p>The Primer is written in Rust — a single codebase from the pedagogy engine down to the device integration — and licensed under the AGPL. For a product whose users are children, "trust us" is not an acceptable privacy model; the only honest answer is source code anyone can read, build, and verify. The knowledge corpus ships with per-passage licensing and attribution, and every claim on this page corresponds to code you can run from the <a href="https://github.com/hherb/primer">public repository</a>.</p>
  </div>
</section>

</main>

<footer class="site-footer">
  <p><a href="mailto:contact@primer-ai.org">contact@primer-ai.org</a> · <a href="https://github.com/hherb/primer">GitHub</a></p>
  <p class="fineprint">© 2026 The Primer project · Code licensed <a href="https://www.gnu.org/licenses/agpl-3.0.html">AGPL-3.0</a> · This site sets no cookies and loads nothing from third parties.</p>
</footer>

</body>
</html>
```

- [ ] **Step 2: Preview locally**

Run: `curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8901/technology.html`
Expected: `200`. Visually: backend card grid renders 4 cards; lists render with comfortable spacing.

- [ ] **Step 3: Commit**

```bash
git add website/technology.html
git commit -m "feat(website): technology page"
```

---

### Task 6: `roadmap.html` — Roadmap & status

**Files:**
- Create: `website/roadmap.html`

- [ ] **Step 1: Write the complete page**

Create `website/roadmap.html` with exactly this content:

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Roadmap &amp; status — The Primer</title>
<meta name="description" content="Where the Primer stands: the cloud-backed proof of pedagogy is complete, local and on-device NPU inference are in progress, and a dedicated child-friendly device is the goal.">
<link rel="canonical" href="https://primer-ai.org/roadmap">
<link rel="icon" type="image/png" href="assets/emblem.png">
<meta property="og:title" content="Roadmap &amp; status — The Primer">
<meta property="og:description" content="The proof of pedagogy is complete; local inference and speech are landing now; a dedicated child-friendly device is the goal.">
<meta property="og:image" content="https://primer-ai.org/assets/banner.png">
<meta property="og:url" content="https://primer-ai.org/roadmap">
<meta property="og:type" content="article">
<meta name="twitter:card" content="summary_large_image">
<link rel="stylesheet" href="style.css">
</head>
<body>

<header class="site-header">
  <div class="header-inner">
    <a class="brand" href="index.html"><img class="brand-mark" src="assets/emblem.png" alt=""><span>The Primer</span></a>
    <nav class="site-nav">
      <a href="vision.html">Vision</a>
      <a href="technology.html">Technology</a>
      <a href="roadmap.html" aria-current="page">Roadmap</a>
      <a href="get-involved.html">Get involved</a>
    </nav>
  </div>
</header>

<main class="container">

<div class="page-hero">
  <p class="eyebrow">Roadmap &amp; status</p>
  <h1>Working software first, then deeper</h1>
  <p class="lede">The strategy: get a genuine Socratic conversation working end-to-end fast, then improve every layer — inference, speech, hardware, pedagogy — independently. Each phase below produces something a child can actually use.</p>
</div>

<section class="section">
  <div class="phase">
    <h3>Phase 0 — Proof of pedagogy <span class="badge badge-done">Complete</span></h3>
    <p>A text-mode Primer that holds a genuine Socratic conversation on any computer. The exit test — a 15-minute conversation that asks more than it answers, catches parroting, suggests breaks, and remembers last time — is met.</p>
    <ul>
      <li>Streaming conversation with session persistence, resume, and long-term memory</li>
      <li>Curated, licensed children's knowledge corpus (English + German) with tuned hybrid retrieval</li>
      <li>Engagement, concept, and comprehension classifiers feeding a persistent learner model</li>
      <li>Spaced-repetition vocabulary and session-break suggestions</li>
      <li>Desktop app, multilingual prompt packs (English/German production, Hindi preview)</li>
    </ul>
  </div>

  <div class="phase">
    <h3>Phase 1 — Local inference <span class="badge badge-progress">In progress</span></h3>
    <p>Run the conversation loop offline on hardware families own. Target: under three seconds to first token on at least one local platform.</p>
    <ul>
      <li>Embedded local inference on laptop/desktop — landed; device benchmarking under way</li>
      <li>Qualcomm NPU backend — pipeline validated on a Snapdragon 8 Elite phone at ~9.4 tokens/s (June 2026); final integration rides with app packaging</li>
      <li>Per-turn hybrid routing between a local model and an optional cloud model — landed, opt-in</li>
    </ul>
  </div>

  <div class="phase">
    <h3>Phase 2 — Speech <span class="badge badge-progress">In progress</span></h3>
    <p>Talk to the Primer instead of typing. Much of this landed ahead of schedule.</p>
    <ul>
      <li>Offline voice loop (voice detection → transcription → response → synthesis) — working today</li>
      <li>Strict turn-taking: no barge-in either direction, by pedagogical design</li>
      <li>Native Apple speech engines on macOS; multilingual synthesis incl. Hindi in evaluation</li>
      <li>Ahead: ambient-noise robustness, echo cancellation, voice-profile selection</li>
    </ul>
  </div>

  <div class="phase">
    <h3>Phase 3 — A device a child can hold <span class="badge badge-progress">Started</span></h3>
    <p>The Primer as a physical object: turn it on and talk, no other equipment. Android app packaging — the deployment path to phone-class hardware — has begun.</p>
    <ul>
      <li>Android app build working; on-device NPU bring-up in final debugging</li>
      <li>Ahead: display (colour e-ink or repurposed tablet), microphone array and speaker, battery and drop-resistant enclosure</li>
    </ul>
  </div>

  <div class="phase">
    <h3>Phase 4 — Pedagogical depth <span class="badge badge-ahead">Ahead</span></h3>
    <p>From a Socratic chatbot to a genuinely effective long-term learning companion.</p>
    <ul>
      <li>Curriculum alignment (Australian Curriculum, IB PYP)</li>
      <li>Multi-session learning arcs that build over weeks</li>
      <li>Read-only parental insight — never surveillance</li>
      <li>Collaborative mode: two children sharing one Primer</li>
      <li>Opt-in, parent-consented, anonymised language corpus to improve child-calibrated models — with on-device scrubbing before anything leaves</li>
    </ul>
  </div>
</section>

<section class="section">
  <h2>Dated milestones</h2>
  <div class="milestone">
    <time datetime="2026-05-26">26 May 2026</time>
    <p>Full text-mode Primer validated end-to-end on an Android phone (Snapdragon 8 Elite) — conversation, persistence, and the complete classifier chain.</p>
  </div>
  <div class="milestone">
    <time datetime="2026-06-09">9 June 2026</time>
    <p>On-device NPU inference validated: a 4-billion-parameter model on the phone's Hexagon neural processor at ~9.4 tokens/s, ~190 ms to first token, 57 °C peak.</p>
  </div>
  <div class="milestone">
    <time datetime="2026-06-11">11 June 2026</time>
    <p>Android app packaging landed: the Primer's desktop app builds and runs as an Android APK, carrying the NPU runtime — the path to the first fully self-contained phone deployment.</p>
  </div>
</section>

</main>

<footer class="site-footer">
  <p><a href="mailto:contact@primer-ai.org">contact@primer-ai.org</a> · <a href="https://github.com/hherb/primer">GitHub</a></p>
  <p class="fineprint">© 2026 The Primer project · Code licensed <a href="https://www.gnu.org/licenses/agpl-3.0.html">AGPL-3.0</a> · This site sets no cookies and loads nothing from third parties.</p>
</footer>

</body>
</html>
```

- [ ] **Step 2: Preview locally**

Run: `curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8901/roadmap.html`
Expected: `200`. Visually: five phase blocks with green/amber/grey badges, then the dated-milestone list.

- [ ] **Step 3: Commit**

```bash
git add website/roadmap.html
git commit -m "feat(website): roadmap & status page"
```

---

### Task 7: `get-involved.html` — Get involved

**Files:**
- Create: `website/get-involved.html`

- [ ] **Step 1: Write the complete page**

Create `website/get-involved.html` with exactly this content:

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Get involved — The Primer</title>
<meta name="description" content="The Primer is looking for educators and researchers to evaluate the pedagogy, funders and hardware partners, native-speaker reviewers, and open-source contributors.">
<link rel="canonical" href="https://primer-ai.org/get-involved">
<link rel="icon" type="image/png" href="assets/emblem.png">
<meta property="og:title" content="Get involved — The Primer">
<meta property="og:description" content="For educators, researchers, funders, hardware partners, translators, and developers.">
<meta property="og:image" content="https://primer-ai.org/assets/banner.png">
<meta property="og:url" content="https://primer-ai.org/get-involved">
<meta property="og:type" content="website">
<meta name="twitter:card" content="summary_large_image">
<link rel="stylesheet" href="style.css">
</head>
<body>

<header class="site-header">
  <div class="header-inner">
    <a class="brand" href="index.html"><img class="brand-mark" src="assets/emblem.png" alt=""><span>The Primer</span></a>
    <nav class="site-nav">
      <a href="vision.html">Vision</a>
      <a href="technology.html">Technology</a>
      <a href="roadmap.html">Roadmap</a>
      <a href="get-involved.html" aria-current="page">Get involved</a>
    </nav>
  </div>
</header>

<main class="container">

<div class="page-hero">
  <p class="eyebrow">Get involved</p>
  <h1>Who we want to hear from</h1>
  <p class="lede">The Primer is an open project. The conversation engine works; making it a genuinely effective learning companion for real children needs people who know children, learning, and hardware better than code.</p>
</div>

<section class="section">
  <div class="grid">
    <div class="card">
      <h3>Educators &amp; researchers</h3>
      <p>Evaluate the pedagogy. Challenge the Socratic implementation, the comprehension probes, the engagement model. Help design pilots and studies with real learners — the project needs evidence, not just conviction.</p>
    </div>
    <div class="card">
      <h3>Funders &amp; partners</h3>
      <p>Support independent, privacy-first educational technology: grants, study funding, hardware for testing, or partnership on a dedicated child-friendly device. The entire stack is open — what you fund stays public.</p>
    </div>
    <div class="card">
      <h3>Native speakers</h3>
      <p>The Primer is multilingual by architecture. Hindi is in preview awaiting native-speaker review, and every new language is data, not development. Help bring the Primer to children in your language.</p>
    </div>
    <div class="card">
      <h3>Developers</h3>
      <p>Rust, on-device inference, speech pipelines, knowledge curation — the codebase is AGPL and the issues are public. Start at the <a href="https://github.com/hherb/primer">GitHub repository</a>.</p>
    </div>
  </div>
</section>

<section class="section">
  <h2>Contact</h2>
  <div class="prose">
    <p>For collaboration, research, funding, or anything else: <a href="mailto:contact@primer-ai.org"><strong>contact@primer-ai.org</strong></a></p>
    <p>For technical discussion, bug reports, and contributions: <a href="https://github.com/hherb/primer">github.com/hherb/primer</a></p>
  </div>
</section>

</main>

<footer class="site-footer">
  <p><a href="mailto:contact@primer-ai.org">contact@primer-ai.org</a> · <a href="https://github.com/hherb/primer">GitHub</a></p>
  <p class="fineprint">© 2026 The Primer project · Code licensed <a href="https://www.gnu.org/licenses/agpl-3.0.html">AGPL-3.0</a> · This site sets no cookies and loads nothing from third parties.</p>
</footer>

</body>
</html>
```

- [ ] **Step 2: Preview locally**

Run: `curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8901/get-involved.html`
Expected: `200`. Visually: four audience cards, contact section with mailto link.

- [ ] **Step 3: Commit**

```bash
git add website/get-involved.html
git commit -m "feat(website): get-involved page"
```

---

### Task 8: `404.html`

**Files:**
- Create: `website/404.html`

- [ ] **Step 1: Write the complete page**

Create `website/404.html` with exactly this content (Cloudflare Pages serves a root-level `404.html` automatically for unknown paths):

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Page not found — The Primer</title>
<meta name="robots" content="noindex">
<link rel="icon" type="image/png" href="/assets/emblem.png">
<link rel="stylesheet" href="/style.css">
</head>
<body>

<header class="site-header">
  <div class="header-inner">
    <a class="brand" href="/"><img class="brand-mark" src="/assets/emblem.png" alt=""><span>The Primer</span></a>
    <nav class="site-nav">
      <a href="/vision.html">Vision</a>
      <a href="/technology.html">Technology</a>
      <a href="/roadmap.html">Roadmap</a>
      <a href="/get-involved.html">Get involved</a>
    </nav>
  </div>
</header>

<main>
<section class="hero">
  <img class="emblem" src="/assets/emblem.png" alt="">
  <p class="eyebrow">404 — page not found</p>
  <h1>This page hasn't been written yet</h1>
  <p class="lede">A good question, but the Primer has no answer at this address.</p>
  <div class="cta-row">
    <a class="btn" href="/">Back to the start</a>
  </div>
</section>
</main>

<footer class="site-footer">
  <p><a href="mailto:contact@primer-ai.org">contact@primer-ai.org</a> · <a href="https://github.com/hherb/primer">GitHub</a></p>
  <p class="fineprint">© 2026 The Primer project · Code licensed <a href="https://www.gnu.org/licenses/agpl-3.0.html">AGPL-3.0</a> · This site sets no cookies and loads nothing from third parties.</p>
</footer>

</body>
</html>
```

Note: `404.html` uses **absolute** paths (`/style.css`, `/assets/emblem.png`) because Cloudflare serves it at arbitrary nested URLs (`/foo/bar`), where relative paths would break.

- [ ] **Step 2: Preview locally**

Run: `curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8901/404.html`
Expected: `200` (locally it's just a page; Cloudflare wires it to actual 404s).

- [ ] **Step 3: Commit**

```bash
git add website/404.html
git commit -m "feat(website): styled 404 page"
```

---

### Task 9: `website/README.md` — deployment instructions

**Files:**
- Create: `website/README.md`

- [ ] **Step 1: Write the README**

Create `website/README.md` with exactly this content:

````markdown
# primer-ai.org website

The public project website, served by [Cloudflare Pages](https://pages.cloudflare.com/) at https://primer-ai.org/.

Plain static HTML + one stylesheet. No build step, no JavaScript, no external requests
(no webfonts, no analytics, no CDNs) — the site practices the project's own privacy principles.

Design spec: `docs/superpowers/specs/2026-06-12-primer-ai-org-website-design.md`.

## Local preview

```bash
python3 -m http.server 8901 -d website
# open http://localhost:8901/
```

## Editing

- Each page is self-contained HTML; the header/nav and footer are repeated verbatim on every
  page (no templating, by design). If you change them, change them on **all six** pages:
  `index.html`, `vision.html`, `technology.html`, `roadmap.html`, `get-involved.html`, `404.html`.
- Internal links use the `.html` form so local preview works; Cloudflare Pages redirects them
  to clean URLs (`/vision`) in production. Canonical/OG URLs use the clean form.
- `404.html` uses absolute paths (`/style.css`) because it's served at arbitrary URLs.
- All colors/typography live in `style.css` under `:root`.

## One-time Cloudflare setup (owner)

### 1. Create the Pages project

1. Cloudflare dashboard → **Workers & Pages** → **Create** → **Pages** → **Connect to Git**.
2. Select the `hherb/primer` repository (authorize the Cloudflare GitHub app if prompted).
3. Build settings:
   - **Production branch:** `main`
   - **Framework preset:** None
   - **Build command:** *(leave empty)*
   - **Build output directory:** `/`
   - **Root directory (advanced):** `website`
4. Save and deploy. Every push to `main` that touches `website/` auto-deploys;
   PR branches get free preview URLs.

### 2. Attach the custom domains

1. In the Pages project → **Custom domains** → **Set up a custom domain**.
2. Add `primer-ai.org`, then repeat for `www.primer-ai.org`.
3. Since the domain is registered with Cloudflare, DNS records are created automatically —
   just confirm. HTTPS certificates are provisioned automatically.

### 3. Email routing (contact@primer-ai.org)

1. Cloudflare dashboard → select the `primer-ai.org` zone → **Email** → **Email Routing**.
2. Click **Get started** / enable. Cloudflare adds the required MX + TXT records automatically.
3. **Destination addresses:** add your personal inbox address; Cloudflare sends a
   verification email — click the link in it.
4. **Routing rules:** create rule `contact@primer-ai.org` → forward to the verified address.
5. Test: send a mail to contact@primer-ai.org from another account and confirm it arrives.
````

- [ ] **Step 2: Commit**

```bash
git add website/README.md
git commit -m "docs(website): deployment + email-routing instructions"
```

---

### Task 10: Verification pass

**Files:** none created — verification only.

- [ ] **Step 1: Automated link/asset check**

Run from the repo root:

```bash
cd website
fail=0
for f in *.html; do
  for ref in $(grep -oE '(href|src)="[^"]+"' "$f" | sed -E 's/^(href|src)="//; s/"$//'); do
    case "$ref" in
      http*|mailto:*|"#"*) continue ;;
    esac
    ref="${ref#/}"          # 404.html uses absolute paths; check them repo-relative
    ref="${ref%%#*}"
    [ -z "$ref" ] && continue   # bare "/" home link
    [ -f "$ref" ] || { echo "MISSING: $f -> $ref"; fail=1; }
  done
done
[ "$fail" -eq 0 ] && echo "ALL LINKS OK"
cd ..
```

Expected output: `ALL LINKS OK`.

- [ ] **Step 2: Serve and status-check every page**

```bash
python3 -m http.server 8901 -d website &
sleep 1
for p in index.html vision.html technology.html roadmap.html get-involved.html 404.html style.css assets/emblem.png assets/banner.png assets/illustration.png; do
  printf '%s: ' "$p"; curl -s -o /dev/null -w "%{http_code}\n" "http://localhost:8901/$p"
done
kill %1
```

Expected: `200` for every entry.

- [ ] **Step 3: Manual click-through (owner or browser tool)**

Open http://localhost:8901/ and verify:
- Header nav reaches all four subpages from every page; brand link returns home.
- The active page's nav item is underlined gold (`aria-current`).
- Landing page: emblem medallion, six principle cards, navy evidence band, four explore cards.
- Narrow the window to ~375 px: nav wraps, card grids collapse to one column, no horizontal scroll.

- [ ] **Step 4: Final commit (if any fixes were needed) and hand off**

```bash
git add website
git commit -m "fix(website): verification-pass fixes" # only if changes exist
```

Then follow the repository's normal PR flow (`superpowers:finishing-a-development-branch`):
the branch merges to `main`, and once the owner completes the one-time Cloudflare setup in
`website/README.md`, every subsequent push to `main` auto-deploys.

**Post-deploy verification (owner, after Cloudflare setup):**
- https://primer-ai.org/ and https://www.primer-ai.org/ load over HTTPS.
- A bogus path (e.g. /nonexistent) serves the styled 404.
- A link-preview checker (or pasting the URL into a chat app) shows the banner OG image.
- Mail to contact@primer-ai.org arrives in the forwarded inbox.
