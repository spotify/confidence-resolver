"""Confidence OpenFeature provider for Python.

This package provides a local WASM-based flag resolver for Confidence feature flags,
implementing the OpenFeature provider interface.
"""

from confidence.version import __version__
from confidence.provider import ConfidenceProvider
from confidence.materialization import (
    MaterializationStore,
    MaterializationNotSupportedError,
    RemoteMaterializationStore,
    UnsupportedMaterializationStore,
)

__all__ = [
    "__version__",
    "ConfidenceProvider",
    "MaterializationStore",
    "MaterializationNotSupportedError",
    "RemoteMaterializationStore",
    "UnsupportedMaterializationStore",
]
