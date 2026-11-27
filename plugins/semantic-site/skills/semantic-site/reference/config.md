# site.json reference

Paths are relative to `site.json`. Only `title` and, for a useful site, one
collection are required; everything else has a default.

## Site

| Key | Default | Meaning |
|---|---|---|
| `title` | required | Site name: header, `<title>`, JSON-LD `WebSite` and `Dataset` name. |
| `description` | `""` | One or two sentences: home page lede, meta description, `llms.txt`. |
| `baseUrl` | none | Absolute URL the site will live at, ending in `/`. Makes JSON-LD `@id`s, canonical links, `sitemap.xml` and `llms.txt` absolute. |
| `lang` | `"en"` | Page language; also picks the language of RDF literals tagged with several. |
| `kicker` | none | Small line above the home page title. |
| `intro` | none | Markdown or `.html` fragment shown on the home page. |
| `license` | none | `{ "name": "CC BY 4.0", "url": "…" }` or a string. Footer, JSON-LD `license`, OpenAPI. |
| `creator` | none | JSON-LD `creator` of the dataset, e.g. `{ "@type": "Organization", "name": "…" }`. |
| `collections` | `[]` | See below. |
| `pages` | `[]` | `{ "path": "about", "title": "About", "source": "content/about.md", "description": "…" }`; `source` may be Markdown or an HTML fragment, or use `"html"` inline. |
| `links` | `[]` | Footer links: `{ "title": "Source code", "url": "…" }`. |
| `theme` | built-in | `accent`, `accent2`, `accentDeep`, `accentDark`, `accent2Dark`: CSS colours. |
| `favicon` | a dot | Path inside the site (put the file in `static`). |
| `static` | none | Folder copied as-is into the site root. |
| `sparql` | none | `{ "endpoint": "https://…/sparql", "examples": [{ "title": "…", "query": "…" }] }`: adds `run_sparql` and `get_example_queries`. |
| `webmcp.scripts` | `[]` | Site-relative ES modules whose default export `(env) => tools[]` adds tools to every page. |
| `webmcp.untrustedContent` | `true` | Mark tools that return site data with `untrustedContentHint`. |

## Collections

```json
{
  "id": "vocabularies",
  "title": "Vocabularies",
  "description": "…",
  "source": "data/vocabularies.csv",
  "rdfType": "https://schema.org/DefinedTermSet",
  "key": "prefix",
  "label": "name",
  "summary": "description",
  "type": "DefinedTermSet",
  "facets": ["status", "publisher"],
  "columns": ["prefix", "publisher", "year"],
  "sort": "-year",
  "order": ["name", "prefix"],
  "fields": {
    "publisher": { "link": "publishers", "label": "Publisher" },
    "keywords": { "split": ";" },
    "namespace": { "type": "url", "property": "identifier" },
    "internal_note": { "hide": true }
  }
}
```

| Key | Default | Meaning |
|---|---|---|
| `id` | required | URL segment: lowercase letters, digits, `-`. Not `api`, `assets`, `agents` or a page path. |
| `title` | from `id` | Shown in navigation, headings, tool descriptions. |
| `description` | `""` | Collection page lede and card text. |
| `source` | required | The data file. Several collections may share one file (pick each with `rdfType`). |
| `rdfType` | all records | Keep only subjects of this class (full IRI, or the class's local name); a list means any of them. |
| `key` | IRI local name, then row number | Field whose value becomes the item's URL id (lowercased, made URL-safe, de-duplicated). |
| `label` | first of `title`, `name`, `label`, `prefLabel`, `headline`, … | Field used as the item's name. |
| `summary` | first of `description`, `abstract`, `summary`, `comment`, `definition`, … | Field shown under the name; `null` for none. |
| `type` | the item's RDF classes, else `Thing` | schema.org type (a term like `Person`) or a full class IRI, for the JSON-LD `@type`. The item's own RDF classes are added to it. |
| `facets` | auto | Fields offered as filters, with value counts. Auto picks up to three fields with 2–30 distinct values present on at least 30% of items (collections of 5 or more). |
| `columns` | facets + one number or date | Fields shown in the list table (max 4 by default). |
| `sort` | `title` | Default order: a field, `title`, or `-field` for descending. |
| `order` | source order | Fields to show first on item pages. |

Facets, columns and the sort field are the **filterable** fields: they are in the
list JSON and are what `list_items` and `show_items` filter on.

### Fields

| Key | Meaning |
|---|---|
| `label` | Display name. Default: the field name in sentence case (`foundingDate` → "Founding date"). |
| `type` | `text`, `number`, `url`, `iri`, `date`, `boolean`. Default inferred; strings of digits from CSV become numbers, RDF literals keep their datatype. |
| `split` | Separator for several values in one CSV cell. |
| `link` | Collection id. Values are matched to its items by key, id, label or IRI and become links; the target page lists them as "Referenced by". IRIs that name another item link without this. |
| `property` | JSON-LD property: a schema.org term or a full IRI. Default: the RDF predicate, a well-known name (`name`, `description`, `url`, `author`, `datePublished`, …), else a schema.org `PropertyValue` under `additionalProperty`. |
| `hide` | Leave the field out everywhere. |

## Item ids

Ids are stable as long as the key values are: changing `key` changes every URL.
Prefer a real identifier (a prefix, a DOI, an accession number) over a title.
