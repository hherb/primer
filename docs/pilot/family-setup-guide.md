# Setting Up the Primer — A Guide for Families

Thank you for volunteering to try the Primer with your child! This guide walks
you through everything you need to do, from getting an API key to handing the
phone to your child. None of the steps require any programming knowledge —
just follow along in order.

Please also read **parent-consent-note.md**, which explains what data the app
handles and where it goes, before your child starts using the Primer.

## 1. What you're installing

The Primer is a learning companion for children that works a bit differently
from most educational apps: instead of just giving answers, it asks
questions. It's built around the "Socratic method" — when your child says
something, the Primer often responds by asking how they know it, or how they
could check, rather than simply confirming or correcting them. The idea is to
encourage curiosity and independent thinking, not just to deliver facts.

**This is a test build**, not a finished product and not something you'd find
on an app store. You're one of a small number of volunteer families helping
us see how well the Primer teaches in practice — how it asks questions,
notices when a child is confused or tired, and suggests breaks. Things may be
rough around the edges, and your feedback matters.

The Primer needs to talk to an AI model to generate its responses, and for
this test build, that means connecting it to an AI provider using an API key
that you provide. The next section walks you through getting one.

## 2. Getting an API key

The Primer can use either **OpenAI** or **Anthropic** (the maker of Claude) as
its AI provider. You only need an account and a key with **one** of them —
pick whichever you're more comfortable with, or whichever you might already
have an account with. Both companies bill based on how much the app is used,
so the steps below also show you how to set a spending limit so you're never
surprised by the bill.

**Important:** the usage this app generates is billed to **your own**
account with the provider you choose. The Primer project does not pay for
this, and we never see your key or your bill. Conversations are short and
this is light personal use, so costs are typically small — but setting a low
limit (as shown below) means you can relax about it.

### Option A: Anthropic (Claude)

1. Go to **console.anthropic.com** in a web browser and create an account
   (or sign in if you already have one).
2. You'll likely need to add a small amount of credit or a payment method
   before you can create a key — follow the on-screen prompts in the
   Anthropic console.
3. In the console, find **API Keys** (usually under Settings) and create a
   new key. Give it a name like "Primer test" so you can recognize it later.
4. **Copy the key immediately** — it's a long string starting with `sk-ant-`.
   Most providers only show you the full key once, so copy it into a notes
   app or leave the browser tab open until you've pasted it into the Primer
   (step 4 below).
5. While you're in the console, look for **spend limits** or **usage limits**
   and set a low monthly cap (for example, $5–$10). This protects you from
   unexpected charges if the app is used a lot.

### Option B: OpenAI

1. Go to **platform.openai.com** and create an account (or sign in).
2. Add billing details if prompted — OpenAI requires a payment method on
   file for API access.
3. Find **API keys** in the left-hand menu and create a new secret key. Name
   it something like "Primer test".
4. **Copy the key immediately** — it starts with `sk-` and, like Anthropic,
   is usually shown only once.
5. Under **Settings → Limits** (or **Billing → Limits**), set a monthly
   budget limit — again, something small like $5–$10 is plenty for personal
   testing.

Keep the key somewhere safe until you've entered it into the app (next
section covers installing the app; the section after that covers entering
the key).

## 3. Installing the app on the phone

You'll have received a file named something like `Primer-<version>-arm64.apk`
— this is the Android installer for the test build. "arm64" just refers to
the type of phone processor it's built for, which covers the vast majority of
Android phones from the last several years.

1. **Get the file onto the phone.** Transfer `Primer-<ver>-arm64.apk` to the
   phone however is easiest for you — email it to yourself and open the
   attachment on the phone, share it via a messaging app, or copy it over USB
   into the phone's Downloads folder.
2. **Allow installing from this source.** Android blocks installing apps
   from outside the Play Store by default, as a security precaution. When
   you tap the APK file to install it, Android will likely show a prompt
   like *"For your security, your phone is not allowed to install unknown
   apps from this source."* Tap the button in that prompt (often labeled
   **Settings**) and toggle on **Allow from this source** for whichever app
   you used to open the file (your file manager, browser, or messaging app).
   Then go back and tap the APK file again.
3. **Install.** Tap **Install** on the confirmation screen. Android may show
   a warning that the app is unrecognized — this is expected for a test
   build; tap through to continue.
4. Once installed, you'll find **Primer** in your app drawer like any other
   app.

You only need to do this once. If we send you an updated test build later,
you'll repeat these steps with the new APK file (Android will update the
existing app rather than creating a duplicate, as long as the version
increases).

## 4. First-run setup (do this once, before your child uses it)

This part is for the adult to do. Once it's set up, your child won't need to
touch any of these settings.

1. Open the **Primer** app.
2. Tap **Settings**.
3. Open the **Inference backend** section.
4. In the **Backend** dropdown, choose one of:
   - **cloud (Anthropic)** — if you got a key from Anthropic in step 2.
   - **openai-compat (oMLX / LM Studio / vLLM)** — this is also the option
     for a regular **OpenAI** account, since OpenAI's API uses this same
     "OpenAI-compatible" format.
5. Depending on which you chose:
   - **If you chose `cloud (Anthropic)`:** scroll to the **API key (cloud
     only)** section, select **Store inline in config file**, and paste your
     Anthropic key into the **Anthropic API key** field that appears.
   - **If you chose `openai-compat`:** you'll also need to fill in the
     **Server URL** field. For a standard OpenAI account, this is
     `https://api.openai.com/v1`. Then scroll to **Server API key
     (openai-compat)**, select **Store inline in config file**, and paste
     your OpenAI key into the **Server API key** field. You should also set
     the **Model** field — for OpenAI this is a model name such as
     `gpt-4o-mini` (ask whoever gave you this build if you're unsure which
     model to use).
6. Scroll up to the **Learner** section and fill in your child's **Name**,
   **Age**, and **Locale** (their language) — this personalizes how the
   Primer talks to them.
7. Tap **Save** (you'll see options like **Save (next session only)** or
   **Save & start new session** — either is fine for first-time setup).

That's it — the AI connection and your child's profile are now saved on this
phone.

## 5. Handing the phone to your child

Your child can now open the Primer and start exploring. There are two ways
to talk to it:

- **Typing.** Just type a question or something they're curious about, the
  same as texting.
- **Voice.** Tap the microphone button (🎙 **Voice mode**) near the top of
  the screen to talk out loud instead of typing. The **first time** voice
  mode is used, Android will ask for permission to use the microphone — this
  is a normal one-time Android permission prompt, and it needs to be
  accepted for voice mode to work.

Remind your child that the Primer likes to ask questions back — if it asks
"how do you know that?" instead of just answering, that's by design, not a
malfunction.

## 6. Troubleshooting

- **A yellow banner says "Ask a grown-up to set up the AI in Settings
  before you start."** This means no API key has been entered yet, or the
  backend isn't configured. Go back to Section 4 above and complete the
  first-run setup.
- **The Primer replies with an error message instead of a normal answer.**
  This is almost always one of two things:
  1. **The API key is wrong, expired, or the account has hit its spending
     limit.** Double-check the key was copied correctly (no extra spaces),
     and check your provider's dashboard to confirm the key is still active
     and the account has available credit/budget.
  2. **No internet connection.** The app needs an active Wi-Fi or mobile
     data connection to reach the AI provider — conversations are not
     processed on the phone itself in this test build.
- **Voice mode doesn't respond.** Check that microphone permission was
  granted (Android Settings → Apps → Primer → Permissions → Microphone). If
  it was previously denied, you may need to enable it there manually.
- **Still stuck?** Contact whoever gave you this test build — screenshots of
  any error message are very helpful for us to track down the problem.

Thank you again for helping test the Primer. We hope your child enjoys the
experience of being asked good questions as much as we've enjoyed building
something that asks them.
