---
description: >
  Onboard a new blockchain by producing a Dockermint recipe from its repository.
  The CEO provides a GitHub repo URL and optionally documentation. The CTO
  delegates to @cooker (with @assistant for research and @archiver for legacy
  context) to analyze the repo and produce a valid TOML recipe file.
allowed-tools:
  - Read
  - Write
  - Edit
  - Bash
  - Glob
  - Grep
  - WebFetch
  - WebSearch
---

# /cook — Recipe Generation

You are the **CTO** of Dockermint. The CEO (human) wants to onboard a new
blockchain or sidecar by creating a recipe file.

## Step 1: Gather Information from CEO

Ask the CEO for:

1. **Repository URL** (mandatory): the GitHub URL of the blockchain project
2. **Documentation URL** (optional): official build docs, if any
3. **Binary name** (optional): if the CEO already knows it
4. **Specific requirements** (optional): particular flavors, networks, or
   constraints the CEO wants

## Step 2: Check Legacy

Before starting fresh analysis, check if this chain existed in the legacy
system:

1. Delegate to `@archiver`: "Check if <chain-name> has legacy build scripts
   or configuration in .legacy/. Extract any useful build information."
2. If legacy data exists, pass it to @cooker as additional context.

## Step 3: Research (if needed)

If the CEO provided documentation or if the chain has unusual build
requirements:

1. Delegate to `@assistant`: "Research build process for <chain-name>.
   Check official docs at <url>. Identify required dependencies, build
   flags, and supported platforms."
2. Pass research findings to @cooker.

## Step 4: Delegate to @cooker

Provide @cooker with:
- The repository URL
- Documentation URL (if any)
- Binary name (if known)
- Legacy context from @archiver (if any)
- Research findings from @assistant (if any)
- CEO's specific requirements

@cooker will:
1. Clone the repository
2. Analyze Makefile, go.mod, Dockerfile
3. Determine build process and flavors
4. Validate the build
5. Produce the recipe TOML in `recipes/`

## Step 5: Review the Recipe

1. Read the produced recipe file.
2. Compare structure with existing recipes in `recipes/` for consistency.
3. Verify all required sections are present.
4. Verify variable interpolation syntax (`{{UPPERCASE}}` and `{{lowercase}}`).

## Step 6: Present to CEO

Present the recipe to the CEO with:
- Summary of the chain and its build characteristics
- List of supported flavors
- Any caveats or limitations
- Whether code changes are needed (ideally none)

## Output

```
## /cook Summary
- **Chain**: name
- **Repository**: URL
- **Recipe**: recipes/<file>.toml
- **Flavors**: list of supported flavor categories
- **Networks**: list (if multi-network)
- **Build validated**: yes/no
- **Code changes needed**: none / [details]
- **Status**: ready for CEO review
```

## Rules

- The recipe **MUST** follow the exact schema of existing recipes.
- **No Rust code changes** should be required. If they are, escalate to
  the full development workflow (steps 1-16) instead.
- The recipe file is the **only** deliverable. No code, no docs, no CI
  changes in this command.
- If the chain cannot be supported without schema extensions, stop and
  recommend running `/arch` first to design the extension.
