# Neuromorphology — 3D neurons & astrocytes, and a fly connectome

**[▸ Launch in the playground →](playground.html#dataset=neuro-showcase&load=lazy)**
— a small `.rete` you can query in your browser, with the astrocyte meshes
rendering **inline in the results table**.

This is an **experiment**: a demonstrator built from two of the neuroscience
datasets harvested into rete, and the point it makes is about *file formats* as
much as neurons — the difference between a **lossless, queryable graph** and a
**lossy 3D render preview**, kept as two layers instead of one.

## What's in it

Just 30 cells, chosen to be interesting rather than large:

- **3 astrocytes** from the Blue Brain **Neuro-Glia-Vascular** reconstruction
  (Zisis et al. 2021), traced from **FIB-SEM electron microscopy**. Each one
  links (`schema:contentUrl`) to a Draco-compressed **`.glb` surface mesh** that
  the playground renders inline — click 🧊 to rotate it. Astrocytes have a fine
  "spongiform" morphology, sheet- and leaflet-like processes that sit *below the
  resolution of light microscopy*; these EM meshes show detail that the tracing
  skeletons in a database like NeuroMorpho simply cannot capture.
- **27 neurons** from the Janelia **hemibrain** connectome — a connected cluster
  of the *Drosophila* **mushroom body**, the fly's learning-and-memory circuit.
  Each carries its cell type and predicted **neurotransmitter**, and they are
  joined by **30 real weighted synaptic edges**. The synapse count on each
  connection is attached to the edge itself with **RDF-star**
  (`<< ?a neuro:connectsTo ?b >> neuro:weight ?w`), so an edge is a first-class
  thing you can query, not a flattened triple.

## The point: lossless graph + lossy preview

The astrocyte meshes are enormous — one is 10 million triangles, ~400 MB as raw
OBJ. That geometry is **not** what belongs in the graph. So the dataset keeps
two layers:

- the **`.rete`** holds every fact **losslessly** — the cells, their types,
  neurotransmitters, morphometrics, the connectome, and the *URL* of each mesh;
- the **`.glb`** is a **lossy preview** for rendering only: decimated to 15 % of
  its triangles and Draco-quantised (a few-nanometre grid, well below the
  imaging resolution), ~8–17 MB instead of hundreds.

The full-resolution OBJ meshes and SWC skeletons stay as the analytical source
of truth. Draco-GLB is a wonderful *delivery* format for a browser, but it is
lossy and it cannot express a graph — so it rides alongside the `.rete`, never
replaces it.

## Try it

Open the playground and run the built-in examples:

- **🧊 The astrocytes in 3D** — returns each astrocyte with its mesh URL; click
  🧊 to open the EM reconstruction and rotate it.
- **Heaviest synapses in the circuit** — reads the RDF-star edge weights; the
  giant modulatory neurons DPM and APL dominate.
- **Neurons by neurotransmitter** — the acetylcholine / GABA balance of the
  mushroom body.
- **The connectome cluster as a network** — switch Output to Graph to see the
  wiring draw itself.

## Licence & credit

Mixed, and published here as a **non-commercial research demonstrator with
attribution**:

- astrocytes — **CC BY-NC-SA 4.0**, © BBP/EPFL; cite Zisis et al. 2021,
  *Digital Reconstruction of the Neuro-Glia-Vascular Architecture*, Cerebral
  Cortex 31(12):5686–5703, [doi:10.1093/cercor/bhab254](https://doi.org/10.1093/cercor/bhab254);
- hemibrain neurons — **CC BY 4.0**; cite Scheffer et al. 2020,
  *A connectome and analysis of the adult Drosophila central brain*, eLife.

Both licences and the citations travel inside the file's dataset card
(`rete card`).
