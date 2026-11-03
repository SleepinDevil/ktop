# ktop — Agent Instructions

Full repository guidelines (project overview, architecture & data flow, key directories, development commands, code conventions, important files, runtime/tooling, testing & QA): see **AGENTS.md**.

Changelog history lives in **CHANGELOG.md** (canonical, newest-first). Do not duplicate it here.

## Mandatory rules

Add changes you make to the changelog including tests you run.

After making changes to Rust source in ktop-rs/, rebuild with:
  cd ktop-rs && cargo build --release
Then copy the binary to install locally:
  cp ktop-rs/target/release/ktop ~/.local/bin/ktop

**CRITICALLY IMPORTANT INSTRUCTIONS BELOW THIS LINE**

**You must NEVER PUSH TO GITHUB without explicitly asking the user!**
