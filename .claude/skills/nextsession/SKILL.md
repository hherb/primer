---
name: nextsession  
description: instructions what to do when starting the next sessions
disable-model-invocation: false
allowed-tools: Bash(git *) Bash(cargo test *) Bash(uv run pytest *)
---
read NEXT_SESSION.md and follow its instructions. 
If you have any questions, ask me
Remember our general coding principles:
- test driven development
- no magic numbers
- prefer pure functions in reusable modules over complex code
- proper inline documentation and unit tests mandatory
- try and keep code files under 500 lines where reasonably possible, refactor when files grow to big where reasonable
Once you have completed yor tasks, check
(a) does README.md need updating because of the changes made in this session? If so, update README.md
(b) does ROADMAP.md need updating because of the changes made in this session? If so, update ROADMAP.md
Before we end, update or create NEXT_SESSION.md containing: 
(1) what we shipped this session with commit SHAs, 
(2) what's next with concrete acceptance criteria, 
(3) any open decisions or risks, 
(4) the exact commands needed to resume (cd, branch, test command). 
(5) Save an EXACT copy of NEXT_SESSION.md to docs/handoffs/ with a timestamp as filename — same content, just a frozen archive for audit/learning. Do not author the handoff separately; NEXT_SESSION.md is the live ephemeral version and the handoff is its timestamped snapshot.