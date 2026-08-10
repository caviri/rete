# Case Studies & Showcases

This section documents real-world experiments and capabilities enabled by `rete`. Whether it's rendering 3D knowledge graphs or using AI agents to extract structured ontology data from voice transcripts, `rete` can handle it.

## Showcase 1: Z-Anatomy (The Human Body as a 3D Graph)

**[▸ Open the 3D body explorer →](anatomy.html)**

We converted the `z-anatomy` knowledge graph into a queryable `.rete` file. By attaching real 3D bounding boxes and coordinates to anatomical structures, the human body becomes a spatial graph.

<figure class="fig-center">
  <img src="img/anatomy-guide.png" alt="The Z-Anatomy 3D explorer">
  <figcaption>The explorer renders the skeleton, cardiovascular system and viscera together in 3D, colour-coding a picked structure's neighbours by how they relate to it.</figcaption>
</figure>

### How it works:
- **Materialized Spatial Edges:** Relations like "adjacent in 3D" or "same tissue" aren't hardcoded visual tricks. They are computed via **GeoSPARQL** 3D extensions (`geof3:distance3D`, `geo3:adjacent3D`) and stored as standard RDF triples.
- **Querying Anatomy:** Asking "What structures touch the stomach?" or "What is within 60mm of the liver?" is just a standard SPARQL query running natively in the browser against the `.rete` file.

## Showcase 2: Extracting Fallacies via AI Agents

An agent connected to the **rete MCP server** can author knowledge graphs, not just read them. In this experiment, we instructed an AI agent to listen to a transcribed debate and construct a validated `.rete` graph of the logical fallacies used by the speakers.

### 1. The Input Transcript
The agent receives a JSON transcript of a debate regarding budget cuts, where participants begin using *ad hominem* and *straw man* arguments.

### 2. Drafting and Linting the Ontology
The agent automatically searches Linked Open Vocabularies, discovers the **Argument Interchange Format (AIF)**, and drafts an ontology. 
The agent runs `check_ontology`, which catches errors (e.g., missing labels, undefined classes). The agent fixes them until the ontology is clean:
```ttl
fal:AdHominem a owl:Class ; rdfs:subClassOf fal:Fallacy ; rdfs:label "Ad hominem" .
fal:StrawMan  a owl:Class ; rdfs:subClassOf fal:Fallacy ; rdfs:label "Straw man" .
```

### 3. Extracting Instances
The agent extracts the specific utterances and maps them to the fallacies:
```ttl
ex:u2 a fal:Utterance ; fal:saidBy ex:ana ; fal:text "We can't trust Bruno — he failed economics twice." .
ex:f1 a fal:AdHominem ; fal:quotes ex:u2 ; fal:attacks ex:u1 .
```

### 4. Building the `.rete` File
The agent runs `build_rete()` to package the ontology and instances into a real `.rete` file.
Within seconds, the file is ready, hosted ephemerally, and queryable.

### 5. Querying and Reasoning
Because the file was built with reasoning, we can ask for general fallacies, and the engine correctly infers that `AdHominem` and `StrawMan` instances belong to the `Fallacy` parent class.
```sparql
# Returns 2 results because the engine understands subclass inheritance
SELECT (COUNT(DISTINCT ?f) AS ?n) WHERE { ?f a fal:Fallacy }
```

### 6. SHACL Validation
The agent ensures data quality by generating a SHACL contract: *"Every annotated fallacy must cite the utterance that commits it."*
Running `validate_shacl` confirms the data conforms to the rules.

### Beyond Fallacies
This pattern—**Speech → Ontology → Instances → Validated Graph**—can be applied anywhere:
- **Meeting Minutes** → Extracting decisions, owners, and deadlines.
- **Interviews** → Extracting historical timelines and cross-referencing them.
- **Clinical Transcripts** → Mapping spoken symptoms to clinical vocabularies.
