## How this site was made

```sh
node skills/semantic-site/scripts/build.mjs example/site.json --out example/dist
node skills/semantic-site/scripts/serve.mjs example/dist
```

`site.json` names the two collections, which file each comes from, which
field is the label, which fields are facets, and that `publisher` links to the
Publishers collection. Everything else is inferred from the data.

## What a page offers

- **Content**: the HTML you are reading, with every list also rendered without JavaScript.
- **Linked data**: schema.org JSON-LD in every page, and all of it in `api/dataset.jsonld`.
- **A static API**: `api/index.json` describes the site; each collection and item is a JSON file.
- **WebMCP tools**: see the [Agents & API](../agents/) page.

The facts here are deliberately few and checkable: each vocabulary's namespace,
its specification, and the year its first version was published.
