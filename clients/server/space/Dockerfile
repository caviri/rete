# The Hugging Face Space (katospiegel/rete) runs the published image as is.
# Push this folder to the Space repo; a factory rebuild pulls the newest
# release.
FROM ghcr.io/caviri/rete-server:latest

# A public server: no token, and no authoring, so nobody can upload files or
# build datasets on it. Queries, MCP, the graph view, skills and tasks are open.
# For a private copy, set RETE_TOKEN as a Space secret and drop RETE_AUTHOR=0.
ENV RETE_PUBLIC_URL=https://katospiegel-rete.hf.space \
    RETE_AUTHOR=0
