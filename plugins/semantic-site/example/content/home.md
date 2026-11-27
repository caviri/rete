This site is built from two files: a spreadsheet of vocabularies
(`data/vocabularies.csv`) and a small RDF graph of their publishers
(`data/publishers.nt`). The publisher column of the spreadsheet links each
vocabulary to its publisher, so every publisher page lists its vocabularies.

Each page is also JSON under `api/`, carries schema.org JSON-LD, and registers
WebMCP tools: an agent in your browser can search, read and filter what you see.
