"""Wikipedia-shaped ingest pipeline submodules.

Split out of the original ``simple_wikipedia.py`` to keep individual
files focused and under the 500-line project guideline. Three modules:

- :mod:`wiki.source` -- domain model and identity (``WikiSource`` config
  dataclass, the ``SIMPLE_ENGLISH`` and ``KLEXIKON`` presets, slug
  helpers, whitelist parser, ``to_passage`` emitter).
- :mod:`wiki.strip` -- wikitext to plain text for the Klexikon fetch
  strategy. Pure functions, no I/O.
- :mod:`wiki.fetch` -- HTTP-fetch dispatch and per-strategy fetchers.
  All network calls flow through here.

The CLI entry point (``main`` orchestrator + argparse) stays in
``simple_wikipedia.py``. Consumers (including the test suite) import
from the specific submodule that owns the name (e.g.
``from wiki.fetch import fetch_lead``); this package's ``__init__``
deliberately exposes no re-exports, so ``from wiki import KLEXIKON``
will fail.
"""
