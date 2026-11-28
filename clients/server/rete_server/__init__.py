"""rete-server: REST, SPARQL 1.1 Protocol and MCP over a catalog of .rete files."""
from importlib.metadata import PackageNotFoundError, version

try:
    __version__ = version("rete-server")
except PackageNotFoundError:  # running from a source checkout
    __version__ = "0+unknown"
