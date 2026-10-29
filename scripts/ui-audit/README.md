# UI audit tooling

Looks at the real UI the way a person does, so a design change can be checked
instead of trusted. Dev tooling only; nothing here ships.

It drives **Google Chrome in headless mode** (so there is no window on your
screen) through the whole app against a running `photodedup serve`.

```
cd scripts/ui-audit && npm ci
cd ../..
cargo build --release -p photodedup-cli && (cd ui && npm ci && npm run build)

# A server on a throwaway database, and a library to scan (any small folder
# of real photos works; about 50 covering duplicates, quick sequences and a
# video or two is plenty). The server prints the address.
./target/release/photodedup serve /tmp/audit.sqlite3        # note the URL it prints

node scripts/ui-audit/screenshots.mjs  "<url>" <library> <outdir> light wide     # or dark/narrow/medium
node scripts/ui-audit/accessibility.mjs "<url>" <library> light 1280 800
node scripts/ui-audit/performance.mjs   "<url>" light            # needs a big scan already in the database
```

Use a **fresh server and database per run** for the screenshots and the
accessibility audit: a second run on the same folder meets the "you scanned
this before" step and carries decisions over.

- `screenshots.mjs`: every screen and state, saved as PNGs (wide 1440,
  medium 1024, narrow 720).
- `accessibility.mjs`: [axe-core](https://github.com/dequelabs/axe-core) with
  the WCAG 2.0/2.1/2.2 A and AA rules on every screen and state. Automated
  checks catch only part of what matters (reading order, focus order and
  screen-reader wording still need a person).
- `performance.mjs`: times opening a big review, switching category, scrolling
  (frame times), opening a group, and making a decision. Run it twice, with
  and without the server's `<db>.thumbs` cache, to see cold against warm.
