window.RETE_PLAYGROUND_CATALOG = {
  defaultDataset: "scholar",
  families: ["Summary", "Select", "Path", "Aggregate", "Construct"],
  datasets: [
    {
      key: "scholar",
      label: "scholar.rete - synthetic scholarly world",
      description: "250 papers, 137 authors, 36 venues from scripts/synth_graph.py (seed 42): power-law citations, field communities, Zipfian venues, and typed literals."
    },
    {
      key: "scholar-noisy",
      label: "scholar-noisy.rete - same world, 25% noise",
      description: "The same generator at --noise 0.25: rewired citations (incl. temporal violations), missing ORCIDs and ISSNs, and whitespace-mangled titles - for SHACL and data-quality demos."
    },
    {
      key: "citations",
      label: "citations - OpenCitations sample",
      description: "Real citation edges around the AlphaFold paper, enriched with labelled synthetic metadata for richer SPARQL examples."
    },
    {
      key: "typed",
      label: "typed.rete - people and orgs",
      description: "Small typed graph for fast schema, SHACL, and social-query smoke tests."
    },
    {
      key: "deps",
      label: "deps.rete - dependency graph",
      description: "Package dependency graph for impact analysis, transitive reachability, and CVE-style examples."
    }
  ],
  examples: {
    scholar: [
      {
        family: "Summary",
        label: "Predicate totals",
        strategy: "progressive",
        view: "table",
        tip: "Exact predicate counts from the pyramid summary; the triple index is skipped.",
        q: `SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p`
      },
      {
        family: "Select",
        label: "Author profiles",
        view: "table",
        tip: "Names, integer-typed h-index values, and affiliations, highest h-index first.",
        q: `PREFIX ex: <http://ex/>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT ?author ?name ?h ?institution WHERE {
  ?author a ex:Person ;
    foaf:name ?name ;
    ex:hIndex ?h ;
    ex:affiliation ?institution
} ORDER BY DESC(?h) LIMIT 50`
      },
      {
        family: "Select",
        label: "High-novelty papers",
        view: "table",
        tip: "FILTER over an xsd:double literal (noveltyScore is log-normal, so the tail is short).",
        q: `PREFIX ex: <http://ex/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title ?score WHERE {
  ?paper ex:noveltyScore ?score ;
    dct:title ?title .
  FILTER(?score > 2.0)
} ORDER BY DESC(?score)`
      },
      {
        family: "Select",
        label: "High-novelty, split by community",
        strategy: "community",
        view: "table",
        tip: "The split strategy: subject stars evaluate per pyramid community, joins recombine the partials globally, FILTER/ORDER BY semantics intact — identical rows to the whole-index run.",
        q: `PREFIX ex: <http://ex/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title ?score WHERE {
  ?paper ex:noveltyScore ?score ;
    dct:title ?title .
  FILTER(?score > 2.0)
} ORDER BY DESC(?score)`
      },
      {
        family: "Path",
        label: "Citation closure",
        view: "table",
        tip: "Transitive cito:cites+ from one recent paper reaches 73 papers (citations only point backwards in time).",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
SELECT DISTINCT ?reached WHERE { <http://ex/paper/245> cito:cites+ ?reached }`
      },
      {
        family: "Aggregate",
        label: "Most-cited papers",
        view: "table",
        tip: "The preferential-attachment power law: a few papers soak up most citations.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title (COUNT(?citing) AS ?citations) WHERE {
  ?citing cito:cites ?paper .
  ?paper dct:title ?title
} GROUP BY ?title ORDER BY DESC(?citations) LIMIT 10`
      },
      {
        family: "Aggregate",
        label: "Papers per field",
        view: "table",
        tip: "Zipfian field sizes: genomics dominates, the tail is thin.",
        q: `PREFIX ex: <http://ex/>
SELECT ?field (COUNT(?paper) AS ?papers) WHERE {
  ?paper a ex:Paper ;
    ex:hasField ?field
} GROUP BY ?field ORDER BY DESC(?papers)`
      },
      {
        family: "Construct",
        label: "Coauthor ego network",
        view: "graph",
        tip: "Two hops of coauthorship around the busiest hub author.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ex:coauthor ?b } WHERE {
  { <http://ex/author/105> ex:coauthor ?b BIND(<http://ex/author/105> AS ?a) }
  UNION
  { <http://ex/author/105> ex:coauthor ?a . ?a ex:coauthor ?b }
}`
      }
    ],
    "scholar-noisy": [
      {
        family: "Summary",
        label: "Predicate totals",
        strategy: "progressive",
        view: "table",
        tip: "Exact predicate counts from the pyramid summary; the triple index is skipped.",
        q: `SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p`
      },
      {
        family: "Select",
        label: "Mangled titles",
        view: "table",
        tip: "REGEX catches the whitespace mess the noise knob injected into 20 titles.",
        q: `PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?paper ?title WHERE {
  ?paper dct:title ?title .
  FILTER(REGEX(?title, "^  "))
}`
      },
      {
        family: "Select",
        label: "Authors missing ORCID",
        view: "table",
        tip: "NOT EXISTS finds the 16 author records the noise stripped an ORCID from.",
        q: `PREFIX ex: <http://ex/>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT ?author ?name WHERE {
  ?author a ex:Person ;
    foaf:name ?name .
  FILTER NOT EXISTS { ?author ex:orcid ?orcid }
}`
      },
      {
        family: "Path",
        label: "Noise-inflated closure",
        view: "table",
        tip: "The same seed paper reaches 16 papers in the clean graph - rewired citations inflate it to 228 here.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
SELECT DISTINCT ?reached WHERE { <http://ex/paper/249> cito:cites+ ?reached }`
      },
      {
        family: "Aggregate",
        label: "Temporal violations",
        view: "table",
        tip: "Papers citing later-dated papers - impossible without noise; 298 of them here.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT (COUNT(*) AS ?violations) WHERE {
  ?citing cito:cites ?cited .
  ?citing dct:date ?d1 .
  ?cited dct:date ?d2 .
  FILTER(STR(?d2) > STR(?d1))
}`
      },
      {
        family: "Aggregate",
        label: "Cross-field citation pairs",
        view: "table",
        tip: "Citations that jump fields are mostly noise rewires; the clean graph keeps them rare.",
        q: `PREFIX ex: <http://ex/>
PREFIX cito: <http://purl.org/spar/cito/>
SELECT ?from ?to (COUNT(*) AS ?n) WHERE {
  ?a cito:cites ?b .
  ?a ex:hasField ?from .
  ?b ex:hasField ?to .
  FILTER(?from != ?to)
} GROUP BY ?from ?to ORDER BY DESC(?n) LIMIT 15`
      },
      {
        family: "Construct",
        label: "Cross-field cites from genomics",
        view: "graph",
        tip: "Draws the noise: genomics papers citing into other fields.",
        q: `PREFIX ex: <http://ex/>
PREFIX cito: <http://purl.org/spar/cito/>
CONSTRUCT { ?a ex:crossFieldCite ?b } WHERE {
  ?a cito:cites ?b .
  ?a ex:hasField <http://ex/field/genomics> .
  ?b ex:hasField ?other .
  FILTER(?other != <http://ex/field/genomics>)
}`
      }
    ],
    citations: [
      {
        family: "Summary",
        label: "Count citation edges",
        strategy: "progressive",
        view: "table",
        tip: "Exact cito:cites count from summary metadata only.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
SELECT (COUNT(*) AS ?citationEdges) WHERE { ?s cito:cites ?o }`
      },
      {
        family: "Select",
        label: "Paper titles",
        view: "table",
        tip: "Single-pattern title scan over enriched citation metadata.",
        q: `PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?paper ?title WHERE { ?paper dct:title ?title } LIMIT 50`
      },
      {
        family: "Aggregate",
        label: "Citations per year",
        view: "table",
        tip: "Join real citation edges with year metadata and group by date.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?year (COUNT(?paper) AS ?n) WHERE {
  ?paper cito:cites <https://doi.org/10.1038/s41586-021-03819-2> .
  ?paper dct:date ?year
} GROUP BY ?year ORDER BY ?year`
      },
      {
        family: "Path",
        label: "Collaborator closure",
        view: "table",
        tip: "A large coauthor transitive path from a high-degree author.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?collaborator WHERE {
  <http://ex/author/1235> ex:coauthor+ ?collaborator
} LIMIT 100`
      },
      {
        family: "Construct",
        label: "Hub ego network",
        view: "graph",
        tip: "Constructs one author's direct coauthor network for the graph renderer.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { <http://ex/author/1235> ex:coauthor ?b } WHERE {
  <http://ex/author/1235> ex:coauthor ?b
}`
      }
    ],
    typed: [
      {
        family: "Summary",
        label: "Count knows edges",
        strategy: "progressive",
        view: "table",
        tip: "Exact social-edge count from summary predicate totals.",
        q: `PREFIX ex: <http://ex/>
SELECT (COUNT(*) AS ?knowsEdges) WHERE { ?s ex:knows ?o }`
      },
      {
        family: "Select",
        label: "Who works where",
        view: "table",
        tip: "Lists employment edges between people and organizations.",
        q: `PREFIX ex: <http://ex/>
SELECT ?person ?org WHERE { ?person ex:worksAt ?org }`
      },
      {
        family: "Select",
        label: "Who knows whom",
        view: "table",
        tip: "Lists direct social links.",
        q: `PREFIX ex: <http://ex/>
SELECT ?a ?b WHERE { ?a ex:knows ?b }`
      },
      {
        family: "Construct",
        label: "Social graph",
        view: "graph",
        tip: "Draws the knows and worksAt graph.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ?p ?b } WHERE {
  { ?a ex:knows ?b BIND(ex:knows AS ?p) }
  UNION
  { ?a ex:worksAt ?b BIND(ex:worksAt AS ?p) }
}`
      }
    ],
    deps: [
      {
        family: "Summary",
        label: "Count dependency edges",
        strategy: "progressive",
        view: "table",
        tip: "Exact dependsOn count from the summary.",
        q: `PREFIX ex: <http://ex/>
SELECT (COUNT(*) AS ?dependencyEdges) WHERE { ?s ex:dependsOn ?o }`
      },
      {
        family: "Select",
        label: "Direct dependencies",
        view: "table",
        tip: "Lists direct package dependency edges.",
        q: `PREFIX ex: <http://ex/>
SELECT ?package ?dependency WHERE { ?package ex:dependsOn ?dependency } LIMIT 100`
      },
      {
        family: "Path",
        label: "Blast radius of log4x",
        view: "table",
        tip: "Packages transitively depending on the vulnerable component.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?dependent WHERE { ?dependent ex:dependsOn+ ex:log4x }`
      },
      {
        family: "Aggregate",
        label: "Dependencies per package",
        view: "table",
        tip: "Direct dependency fan-out by package.",
        q: `PREFIX ex: <http://ex/>
SELECT ?package (COUNT(?dependency) AS ?deps) WHERE {
  ?package ex:dependsOn ?dependency
} GROUP BY ?package ORDER BY DESC(?deps)`
      },
      {
        family: "Construct",
        label: "Dependency graph",
        view: "graph",
        tip: "Draws the dependency graph.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ex:dependsOn ?b } WHERE { ?a ex:dependsOn ?b }`
      }
    ]
  },
  shacl: {
    scholar: [
      {
        label: "Paper integrity",
        tip: "Every paper needs exactly one title, a venue, and a double-typed novelty score - the clean graph conforms.",
        shape: `@prefix ex: <http://ex/> .
@prefix dct: <http://purl.org/dc/terms/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:PaperShape
  a sh:NodeShape ;
  sh:targetClass ex:Paper ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ; sh:maxCount 1 ] ;
  sh:property [ sh:path ex:publishedIn ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:noveltyScore ; sh:datatype xsd:double ] .`
      },
      {
        label: "Single keyword only",
        tip: "Intentional violation: papers carry 2-5 keywords by design, so maxCount 1 flags all 240 of them.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:SingleKeywordShape
  a sh:NodeShape ;
  sh:targetClass ex:Paper ;
  sh:property [
    sh:path ex:keyword ;
    sh:maxCount 1 ;
    sh:message "Papers are multi-keyword by design - intentional violation."
  ] .`
      }
    ],
    "scholar-noisy": [
      {
        label: "Author completeness",
        tip: "The noise knob dropped ORCIDs and h-indexes - 27 violations here; the clean graph conforms.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonCompleteShape
  a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path ex:orcid ; sh:minCount 1 ; sh:message "Author has no ORCID." ] ;
  sh:property [ sh:path ex:hIndex ; sh:minCount 1 ; sh:message "Author has no h-index." ] .`
      },
      {
        label: "Journals need an ISSN",
        tip: "Four journals lost their ISSN to the noise knob.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:JournalShape
  a sh:NodeShape ;
  sh:targetClass ex:Journal ;
  sh:property [
    sh:path ex:issn ;
    sh:minCount 1 ;
    sh:message "Journal lost its ISSN."
  ] .`
      }
    ],
    citations: [
      {
        label: "Citing paper metadata",
        tip: "Checks title, date, and discipline for papers with citation edges.",
        shape: `@prefix cito: <http://purl.org/spar/cito/> .
@prefix dct: <http://purl.org/dc/terms/> .
@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:CitingPaperMetadataShape
  a sh:NodeShape ;
  sh:targetSubjectsOf cito:cites ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ] ;
  sh:property [ sh:path dct:date ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:discipline ; sh:minCount 1 ] .`
      }
    ],
    typed: [
      {
        label: "Employment is an org",
        tip: "Every worksAt value must be an organization.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonEmploymentShape
  a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [
    sh:path ex:worksAt ;
    sh:class ex:Org ;
    sh:maxCount 1
  ] .`
      },
      {
        label: "Every person needs a name",
        tip: "Intentional MinCount violation in the tiny graph.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonNameShape
  a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [
    sh:path ex:name ;
    sh:minCount 1 ;
    sh:message "Every Person must have a name."
  ] .`
      }
    ],
    deps: [
      {
        label: "Application dependencies",
        tip: "The application should have at least one dependency.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:ApplicationDependencyShape
  a sh:NodeShape ;
  sh:targetClass ex:Application ;
  sh:property [
    sh:path ex:dependsOn ;
    sh:minCount 1
  ] .`
      }
    ]
  },
  reach: {
    scholar: {
      pred: "<http://ex/coauthor>",
      seeds: "<http://ex/author/105>",
      examples: [
        { label: "Hub author's coauthor closure", pred: "<http://ex/coauthor>", seeds: "<http://ex/author/105>", reverse: false },
        { label: "Who cites the most-cited paper", pred: "<http://purl.org/spar/cito/cites>", seeds: "<http://ex/paper/15>", reverse: true },
        { label: "Citation closure of paper 245", pred: "<http://purl.org/spar/cito/cites>", seeds: "<http://ex/paper/245>", reverse: false }
      ]
    },
    "scholar-noisy": {
      pred: "<http://purl.org/spar/cito/cites>",
      seeds: "<http://ex/paper/249>",
      examples: [
        { label: "Noise-inflated citation closure", pred: "<http://purl.org/spar/cito/cites>", seeds: "<http://ex/paper/249>", reverse: false },
        { label: "Who cites paper 14 (transitively)", pred: "<http://purl.org/spar/cito/cites>", seeds: "<http://ex/paper/14>", reverse: true },
        { label: "Hub author's coauthor closure", pred: "<http://ex/coauthor>", seeds: "<http://ex/author/120>", reverse: false }
      ]
    },
    citations: {
      pred: "<http://ex/coauthor>",
      seeds: "<http://ex/author/1235>",
      examples: [
        { label: "Author 1235 network", pred: "<http://ex/coauthor>", seeds: "<http://ex/author/1235>", reverse: false },
        { label: "Who cites AlphaFold", pred: "<http://purl.org/spar/cito/cites>", seeds: "<https://doi.org/10.1038/s41586-021-03819-2>", reverse: true }
      ]
    },
    typed: {
      pred: "<http://ex/knows>",
      seeds: "<http://ex/Alice>",
      examples: [
        { label: "Alice knows", pred: "<http://ex/knows>", seeds: "<http://ex/Alice>", reverse: false },
        { label: "Who knows Bob", pred: "<http://ex/knows>", seeds: "<http://ex/Bob>", reverse: true }
      ]
    },
    deps: {
      pred: "<http://ex/dependsOn>",
      seeds: "<http://ex/app>",
      examples: [
        { label: "App dependency closure", pred: "<http://ex/dependsOn>", seeds: "<http://ex/app>", reverse: false },
        { label: "Log4x blast radius", pred: "<http://ex/dependsOn>", seeds: "<http://ex/log4x>", reverse: true }
      ]
    }
  },
  provenance: {
    scholar: { predicate: "<http://purl.org/spar/cito/cites>", object: "<http://ex/paper/15>" },
    "scholar-noisy": { predicate: "<http://purl.org/spar/cito/cites>", object: "<http://ex/paper/14>" },
    citations: { predicate: "<http://purl.org/spar/cito/cites>", object: "<https://doi.org/10.1038/s41586-021-03819-2>" },
    typed: { predicate: "<http://ex/knows>" },
    deps: { predicate: "<http://ex/dependsOn>" }
  }
};
