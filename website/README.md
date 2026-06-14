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
