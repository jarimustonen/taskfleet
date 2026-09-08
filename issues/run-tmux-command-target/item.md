---
created: 2026-08-24
updated: 2026-09-08
type: feature
reporter: jari
status: done
priority: normal
provenance: chat
source_ref: pi-session:01a02ac6-7a1d-7576-80a5-2e1ff864f474/report:cdb30b0c5ca2b4fa3982992bc4ca6196097fb99575b63c7717ac507133719698
closed: 2026-09-08
---

# Expose the correct tmux command target for a run

## Description

Lisätään uusi feature. Pitää olla  `tmux send-keys -t 'headless:xxx' "..."` tyyppistä komennon lähettämistä varten oma työkalu orhcestratectl:ssä niin että se menee oikeaan paikkaan. Tai ehkä -t parametrin oiekan arvon saa esiin orchestartectl:stä. Tästä voisi tehdä issuen

## Resolution

### 2026-09-08T08:40:25Z · @issuectl

Taskfleet now exposes the qualified tmux target needed for precise send-keys routing through its run/node read surfaces.
