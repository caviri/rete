# Football — match replays from a graph

Two interactive demos that replay real football from a **spatiotemporal `.rete` graph** — every player's position, the ball, and the match events are triples, range-fetched as the clock advances. No video: what you see is the knowledge graph rendering itself.

- **[Pick any match →](pitch.html)** — a canvas pitch replaying player + ball positions at 5 fps, with occupancy heatmaps for any player, team, or the ball. Data: StatsBomb open positional feeds, one `.rete` per match.
- **[The 2022 World Cup final →](wcfinal.html)** — Argentina 3–3 France replayed from StatsBomb 360 freeze-frames: every player's place at every recorded moment, a live scoreboard, and jump-to-goal navigation.

## The same data, queryable

The graphs behind these demos are regular playground datasets — open them and ask questions with SPARQL instead of watching:

- [`worldcup`](playground.html#dataset=worldcup&load=lazy) — the whole 2022 tournament: stats, player careers, full squads, multi-source predictions.
- [`worldcup2026`](playground.html#dataset=worldcup2026&load=lazy) — a live, in-progress snapshot of the current cup, cross-checked against live sources.

A replay and a query are the same range reads over the same file — the demos just fetch the positions for the current second, exactly like a `LIMIT`ed SPARQL query would.
