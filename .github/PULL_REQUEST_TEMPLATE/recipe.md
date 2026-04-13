## Description

A clear and concise description of the recipe change.

- Summary of changes
- Reasoning
- Additional context

Closes {LINK TO GH ISSUE}

## Type of change

- [ ] New recipe (add a new chain recipe)
- [ ] Recipe modification (edit an existing recipe file)

## Checks to complete

- [ ] Branch is up-to-date with origin
- [ ] I have performed a self-review of my changes
- [ ] PR title is in a conventional commit style
- [ ] All my commits are GPG signed
- [ ] `CHANGELOG.md` has been updated

## Recipe validation

- [ ] Recipe linted / validated against schema
- [ ] Build a chain node (specify related side-car(s))
- [ ] Build a side-car (specify related node chain)

## Chain node verification

- [ ] Recipe built with default flavors in rootless context
- [ ] Node successfully synced with default flavors built image
- [ ] **NO** wrong app hash nor consensus failure

**MUST** specify:
- Tested chain version: `{SEMVER}`
- Cosmos SDK version: `{SEMVER}`
- CometBFT version: `{SEMVER}`
- Synced from:
    - [ ] Genesis
    - [ ] Native snapshot
    - [ ] Custom snapshot
    - [ ] State sync
- Running environment:
    - OS: `{OS}`
    - CPU: `{CPU}`
    - RAM: `{RAM}`
    - Disk: `{DISK}`
    - Bandwidth: `{BANDWIDTH}`

## Evidence

<details>
<summary>Sync logs / screenshots</summary>

Paste relevant logs, block height reached, sync duration, or screenshots here.

</details>
