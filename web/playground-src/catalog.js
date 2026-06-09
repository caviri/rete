window.RETE_PLAYGROUND_CATALOG = {
  defaultDataset: "research",
  families: ["Summary", "Select", "Path", "Aggregate", "Construct"],
  datasets: [
    {
      key: "research",
      label: "research.rete - academic graph",
      description: "Researcher, paper, institution, journal, coauthor, citation, and advisor relations for the broadest ontology showcase."
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
    },
    {
      key: "papers",
      label: "papers.rete - citation graph",
      description: "Compact paper citation graph with titles and abstracts for community and path examples."
    },
    {
      key: "researchers",
      label: "researchers.rete - coauthorship",
      description: "Multi-criteria researcher graph with coauthor and citation relations."
    }
  ],
  examples: {
    research: [
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
        label: "Researcher profiles",
        view: "table",
        tip: "Names, h-index values, and institutions joined through the academic profile graph.",
        q: `PREFIX ex: <http://ex/>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT ?researcher ?name ?h ?institution WHERE {
  ?researcher a ex:Researcher ;
    foaf:name ?name ;
    ex:hIndex ?h ;
    ex:affiliatedWith ?institution
} ORDER BY DESC(?h) LIMIT 50`
      },
      {
        family: "Path",
        label: "Advisor lineage",
        view: "table",
        tip: "Transitive advisor chain from a known researcher seed.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?mentor WHERE { <http://ex/r/bob> ex:advisedBy+ ?mentor }`
      },
      {
        family: "Aggregate",
        label: "Coauthor degree",
        view: "table",
        tip: "Counts direct coauthors per researcher.",
        q: `PREFIX ex: <http://ex/>
SELECT ?researcher (COUNT(?coauthor) AS ?coauthors) WHERE {
  ?researcher ex:coauthor ?coauthor
} GROUP BY ?researcher ORDER BY DESC(?coauthors)`
      },
      {
        family: "Construct",
        label: "Coauthor graph",
        view: "graph",
        tip: "Constructs a node-link graph of coauthor edges.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ex:coauthor ?b } WHERE { ?a ex:coauthor ?b }`
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
    ],
    papers: [
      {
        family: "Summary",
        label: "Count citation edges",
        strategy: "progressive",
        view: "table",
        tip: "Exact compact citation count from summary metadata.",
        q: `PREFIX ex: <http://ex/>
SELECT (COUNT(*) AS ?citationEdges) WHERE { ?s ex:cites ?o }`
      },
      {
        family: "Select",
        label: "Paper titles",
        view: "table",
        tip: "Lists papers and titles.",
        q: `PREFIX ex: <http://ex/>
SELECT ?paper ?title WHERE { ?paper ex:title ?title }`
      },
      {
        family: "Path",
        label: "Reachable from p1",
        view: "table",
        tip: "Transitive citation reachability from p1.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?reached WHERE { ex:p1 ex:cites+ ?reached }`
      },
      {
        family: "Aggregate",
        label: "Citations made per paper",
        view: "table",
        tip: "Outgoing citation counts.",
        q: `PREFIX ex: <http://ex/>
SELECT ?paper (COUNT(?cited) AS ?cites) WHERE {
  ?paper ex:cites ?cited
} GROUP BY ?paper ORDER BY DESC(?cites)`
      },
      {
        family: "Construct",
        label: "Citation graph",
        view: "graph",
        tip: "Draws paper citation clusters.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ex:cites ?b } WHERE { ?a ex:cites ?b }`
      }
    ],
    researchers: [
      {
        family: "Summary",
        label: "Count coauthor edges",
        strategy: "progressive",
        view: "table",
        tip: "Exact coauthor count from summary metadata.",
        q: `PREFIX ex: <http://ex/>
SELECT (COUNT(*) AS ?coauthorEdges) WHERE { ?s ex:coauthor ?o }`
      },
      {
        family: "Select",
        label: "Coauthorships",
        view: "table",
        tip: "Lists direct coauthor edges.",
        q: `PREFIX ex: <http://ex/>
SELECT ?a ?b WHERE { ?a ex:coauthor ?b } LIMIT 100`
      },
      {
        family: "Path",
        label: "r1 citation neighborhood",
        view: "table",
        tip: "Transitive citation closure from r1.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?reached WHERE { ex:r1 ex:cites+ ?reached }`
      },
      {
        family: "Aggregate",
        label: "Most collaborative",
        view: "table",
        tip: "Direct coauthor count per researcher.",
        q: `PREFIX ex: <http://ex/>
SELECT ?researcher (COUNT(?coauthor) AS ?n) WHERE {
  ?researcher ex:coauthor ?coauthor
} GROUP BY ?researcher ORDER BY DESC(?n)`
      },
      {
        family: "Construct",
        label: "Cites and coauthors",
        view: "graph",
        tip: "Draws the relation mix between citations and coauthorships.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ?p ?b } WHERE {
  { ?a ex:cites ?b BIND(ex:cites AS ?p) }
  UNION
  { ?a ex:coauthor ?b BIND(ex:coauthor AS ?p) }
}`
      }
    ]
  },
  shacl: {
    research: [
      {
        label: "Researcher profile",
        tip: "Checks that every researcher has a name, h-index, and institution.",
        shape: `@prefix ex: <http://ex/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:ResearcherProfileShape
  a sh:NodeShape ;
  sh:targetClass ex:Researcher ;
  sh:property [ sh:path foaf:name ; sh:minCount 1 ; sh:maxCount 1 ] ;
  sh:property [ sh:path ex:hIndex ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:affiliatedWith ; sh:minCount 1 ; sh:class ex:Institution ] .`
      },
      {
        label: "Journal impact integer",
        tip: "Intentional violation if impact factors are decimal-like literals.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:JournalImpactShape
  a sh:NodeShape ;
  sh:targetClass ex:Journal ;
  sh:property [
    sh:path ex:impactFactor ;
    sh:datatype xsd:integer ;
    sh:message "Impact factor must be typed as xsd:integer."
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
    ],
    papers: [
      {
        label: "Paper text fields",
        tip: "Every paper should have title and abstract literals.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PaperTextShape
  a sh:NodeShape ;
  sh:targetClass ex:Paper ;
  sh:property [ sh:path ex:title ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:abstract ; sh:minCount 1 ] .`
      }
    ],
    researchers: [
      {
        label: "Two coauthors required",
        tip: "Intentional violation in the small coauthorship graph.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:ResearcherCoauthorShape
  a sh:NodeShape ;
  sh:targetClass ex:Researcher ;
  sh:property [
    sh:path ex:coauthor ;
    sh:minCount 2 ;
    sh:message "Researcher has fewer than two coauthors."
  ] .`
      }
    ]
  },
  reach: {
    research: {
      pred: "<http://ex/coauthor>",
      seeds: "<http://ex/r/alice>",
      examples: [
        { label: "Alice coauthors", pred: "<http://ex/coauthor>", seeds: "<http://ex/r/alice>", reverse: false },
        { label: "Who cites p3", pred: "<http://ex/cites>", seeds: "<http://ex/p/p3>", reverse: true },
        { label: "Bob advisor lineage", pred: "<http://ex/advisedBy>", seeds: "<http://ex/r/bob>", reverse: false }
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
    },
    papers: {
      pred: "<http://ex/cites>",
      seeds: "<http://ex/p1>",
      examples: [
        { label: "p1 citation closure", pred: "<http://ex/cites>", seeds: "<http://ex/p1>", reverse: false },
        { label: "Who cites p5", pred: "<http://ex/cites>", seeds: "<http://ex/p5>", reverse: true }
      ]
    },
    researchers: {
      pred: "<http://ex/coauthor>",
      seeds: "<http://ex/r1>",
      examples: [
        { label: "r1 coauthors", pred: "<http://ex/coauthor>", seeds: "<http://ex/r1>", reverse: false }
      ]
    }
  },
  provenance: {
    research: { predicate: "<http://ex/coauthor>" },
    citations: { predicate: "<http://purl.org/spar/cito/cites>", object: "<https://doi.org/10.1038/s41586-021-03819-2>" },
    typed: { predicate: "<http://ex/knows>" },
    deps: { predicate: "<http://ex/dependsOn>" },
    papers: { predicate: "<http://ex/cites>" },
    researchers: { predicate: "<http://ex/coauthor>" }
  }
};
