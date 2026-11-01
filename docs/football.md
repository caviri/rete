# Football — match replays from a graph

Two interactive demos display football positions and events as animated data.
The viewers load JSON snapshots and advance through them locally; they do not
query a `.rete` file at each tick of the clock.

- **[Pick any match →](pitch.html)** — a canvas pitch replaying player + ball positions at 5 fps, with occupancy heatmaps for any player, team, or the ball. Data: StatsBomb open positional feeds, one `.rete` per match.
- **[The 2022 World Cup final →](wcfinal.html)** — Argentina 3–3 France replayed from StatsBomb 360 freeze-frames: every player's place at every recorded moment, a live scoreboard, and jump-to-goal navigation.

## The same data, queryable

The graphs behind these demos are regular playground datasets — open them and ask questions with SPARQL instead of watching:

- [`worldcup`](playground.html#dataset=worldcup&load=lazy) — the whole 2022 tournament: stats, player careers, full squads, multi-source predictions.
- [`worldcup2026`](playground.html#dataset=worldcup2026&load=lazy) — a tournament dataset; inspect its card for the source and snapshot date.

The replay pages and the Playground have different data paths: the former
animate downloaded snapshots; the latter runs SPARQL against a graph file.
