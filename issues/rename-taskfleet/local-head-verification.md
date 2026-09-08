# Local repository HEAD identity verification — 2026-09-08

## Result

The user requested one lightweight independent audit per locally owned repository,
including work/organization repositories, permitting retired-name mentions only
in genuinely historical material. Thirty-eight independent read-only agents
inspected the 38 direct repository checkouts under Sources whose remotes identify
the user or the relevant work organizations. Five third-party source clones were
excluded: claude-code, ds4, openclaw, pi, and pi-mono.

The initial audit found 30 clean repositories, four with historical references,
and four with current references. The conductor subsequently launched one
correction worktree spinoff for each of the latter four. All four merged through
`taskfleet run merge`, were verified directly from Git, and were pushed to their
respective remote main branches. No stale current product guidance remains in
those findings. Historical provenance is preserved, as is the retired tap's
intentional formula migration mapping; that mapping is the explicit operational
exception to a literal historical-prose-only criterion.

## Method and limits

Each agent read repository instructions and inspected a pinned Git commit using
case-insensitive retired long-name/abbreviation searches plus tracked-path
inspection. Matching context was read from Git objects. Closed issue history,
frozen run prompts, and dated completed-work accounts were distinguished from
open issue guidance, maintained comments, commands, and configuration. Incidental
encoded data and unrelated tool substrings were excluded. The conductor repeated
the mechanical content/path inventory and directly verified every active finding.

Dirty working trees were left untouched. This verifies tracked local HEAD source,
not Git history, ignored files, installed symlink targets, remote HEAD convergence,
or running services on other hosts. Repository content was not edited, tested,
installed, or released by the audit agents. The durable report is centralized in
this existing issue instead of adding documents or issues to all 38 repositories.

## Initial findings and completed corrections

| Repository | Location | Finding |
| --- | --- | --- |
| aggountant | `issues/cli-canon-s22/item.md:31` | Open, scheduled issue recommends the retired product as a current reference tool. |
| crmctl | `issues/cli-canon-s22/item.md:31` | Open issue recommends the retired product as a current reference tool. |
| crmctl | `issues/cli-canon-skill-subcommand/item.md:28` | Open implementation guidance names the retired product as the current example. |
| glasspad | `crates/glasspad-cli/src/cli/info.rs:10` and `:14` | Maintained Rust documentation uses the retired sibling command and envelope name. |
| glasspad | `dist-workspace.toml:7` and `:17` | Maintained release comments name the retired product and tap as the current convention. |
| glasspad | `tests/version_cli.rs:35` | Current test commentary uses the retired sibling name. |
| retired Homebrew tap | `README.md:1–2` | Present-tense text advertises the retired tap as a current distribution channel. |
| retired Homebrew tap | `tap_migrations.json:2` | Intentional operational mapping from the retired formula key to the canonical Taskfleet formula. This is compatibility metadata, not historical prose or a stale executable invocation. |

The first seven rows are wording follow-ups. The final row is an explicit
migration mechanism and must not be deleted as a blanket text replacement.
The retired tap's inspected tree contains only its README and migration mapping;
it has no formula. Its old local directory/origin name is separately understood
as repository metadata. This audit does not change tap behavior.

## Correction landings — 2026-09-08

| Repository | Merged and pushed commit | Verification |
| --- | --- | --- |
| aggountant | `b157936d016f28da6ba4cc9945f16fae568285ee` | One open-issue prose substitution; frontmatter unchanged. |
| crmctl | `48866aa1d6197385c513b766cdb72bfa6a1b43d7` | Two open-issue prose substitutions; frontmatter unchanged. |
| glasspad | `cd0e55c3a3e0b19d78dd1d95894a282ad9360bc8` | Five comment substitutions across the three allowed files; executable source and configuration unchanged. |
| retired Homebrew tap | `932705f3914cf3541228a263b563ac5ed5f054f3` | README now states retirement and canonical installation; migration mapping retains its exact Git blob. |

Each run has a successful explicit-merge report and `landed: true`. The conductor
reviewed every complete commit diff, verified clean source trees, repeated the
HEAD name scan, and pushed main after a successful rebase check. Remaining
matches in the first three repositories are closed issue history or immutable
run provenance. The tap retains a clearly historical name and the intentional
migration metadata needed by existing installations. These corrections do not
install tools or change release behavior in any dependent repository.

## Per-repository evidence

The table records the exact commit independently inspected by each agent.

| Repository | Inspected commit | Verdict |
| --- | --- | --- |
| 3dbear-monorepo | `9fb23203e7ceb4aed5a2547d09e4e6cb45edb801` | historical-only |
| aggountant | `95d1bb31df4c1e2705d3f53e0fb8110d201ee796` | active-references |
| blog | `eea5a547c2a8d15ed441462f430e8b2e6ddc3141` | clean |
| crmctl | `ca9bcd40eee0c33e949583e1703990678d6b4087` | active-references |
| deutschpad | `6393be56bc9be04318334438911c90e9dcd4bfbf` | clean |
| dkv-thunderbird-plugin | `f736d029227650afd86150ff2e5a59c0f6591ca4` | clean |
| dkv-user-communication | `6b88ae204600a25fcadbf48e6340ef4cda38ec37` | clean |
| dkv-userdb | `816d3b1a8b54defdde38f16a898a39ce584563cb` | clean |
| formative-agent | `4de8f930b55ef30af4fa565530108a24b77b5183` | clean |
| formative-memory | `49507597d8e83dc1b6c7d5149cbf48c498846d8f` | clean |
| formative-memory-maintenance | `d6fe279f0c1979d4da54e560dc787f3ad938fd5b` | clean |
| frondeo-monorepo | `08b6171d7fad8b287e5dbd7bbb5d05a283e09c71` | clean |
| glasspad | `4271f45ac69008293a71331df3e8fe87aa06d012` | active-references |
| grooveserve-monorepo | `145a6de35a8ac2a82c793ad650ff5030f0740c41` | clean |
| homebase | `e28071326976f56b703a5993fa23235457cb751b` | clean |
| retired Homebrew tap | `20a70f463e699af5ddba6f6455c20a183c496ca5` | active-references |
| hyrox-academy | `af69577e70060004eff5d05c0709296716b7e418` | clean |
| intakectl | `d7b5c4441752268096e1f252759cd6ab85131aa9` | historical-only |
| issuectl | `b621beddc08a42c5c1c8b4b1565a1b1b2cae3029` | clean |
| itsellesi-monorepo | `e886a2531f2d5d3e142c46c6d3acec04f48e5295` | clean |
| kunnollavauhtiin-en | `a976f223ca12c9eceb5d4bac12e0e2b27f6dbc34` | clean |
| kunnollavauhtiin-images | `5a9523ad12f1e644a3478affcd5eb5e994f5c12c` | clean |
| kunnollavauhtiin-monorepo | `16f363a4d27277927c20f67d9e0cbc078f30f41e` | clean |
| kunnollavauhtiin5 | `413d7d7ec73ec1d6d088e459b2448a4c56c45c85` | clean |
| okv-email-templates | `ce5acf32a5f8726b1174a7b2110d4608edfc6631` | clean |
| okv-homepage | `f1a7f6d3c9b91e80aa76a1950de3a34069a2e113` | clean |
| okv-monorepo | `71f48042b7917976970aae7b30f1d98ef6a9f10f` | clean |
| okv-submissions-leaning | `8a390c219c1e4c824f47d84e18c34b855aaa1f7d` | clean |
| osa-material-processing | `64b0e832aa7c268c018c49d59c8a0b608f45a8c4` | clean |
| osa-teachers-tool | `81a106957210143f5f46e08cae605a63987af66d` | clean |
| ossctl | `d1d48d692707fee0d98697721e763a59e7ee3fb7` | clean |
| out-of-context | `54742121b5ee6e8b925140ee3af164b44d474c43` | clean |
| project-canon | `454f73bb46ea59b5d00dd0ea1ec7726d1ecc6593` | historical-only |
| reitti-cli | `9c3e18832050152dc9d4e7328a8629090af7c288` | clean |
| simuna-creator | `a2928d9681ac3cc1130c5362347ed6f36fb963d7` | clean |
| taskfleet | `caa79c71044a455f9cd56acd6f0a40ed806d65e4` | clean |
| taskfleet-pi-telemetry | `b19a3b9b3613ea6208c7a60a83d2d9e6327afa18` | clean |
| vensum-workspace | `ab257b0597066f4812a7c2ef2efe2217b9bdf86d` | historical-only |

Historical-only repositories are 3dbear-monorepo, intakectl, project-canon, and
vensum-workspace. Their accepted occurrences belong to closed issues, completed
handoff accounts, or archived run-specific prompts. Historical classification
is not a fresh endorsement of every old technical claim in those documents.

## HEAD movement during inspection

- homebase advanced to `3b3ef9f58f31ab94f1d1b92f3a9a1b59d8c4de9b`. The conductor checked all 6 changed paths and their new content: no new retired-name match. Its verdict remains unchanged.
- intakectl advanced to `5f54fa364feb60e3d0497d4c4377fb126ad8f978`. The conductor checked all 1 changed paths and their new content: no new retired-name match. Its verdict remains unchanged.

All other inspected HEADs were unchanged at the final inventory check.

## Published distribution check

The canonical Homebrew formula declares version **0.7.1**, at tap commit
`979903effc26ce366541e7d5169cc2b18909b605`, updated 2026-09-07 06:08:51 UTC.
GitHub's latest published Taskfleet release is **v0.7.1**, published
2026-09-07 06:07:56 UTC. The installed binary reports **0.7.1**, commit
`b9e15ae784837dd278961360d45839a70d17e2ed`. These observations agree: Homebase's
reported tap version is correct. The preceding regression fixes were pushed to
main without a new version/tag release, so they are not yet distributed by that
formula. A main push does not itself update the tap.

Sources: [pinned canonical formula](https://github.com/jarimustonen/homebrew-taskfleet/blob/979903effc26ce366541e7d5169cc2b18909b605/Formula/taskfleet.rb)
and [published v0.7.1](https://github.com/jarimustonen/taskfleet/releases/tag/v0.7.1).
No installation or release was performed during this verification.
