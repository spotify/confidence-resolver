"""Confidence OpenFeature provider for Python.

This package provides a local WASM-based flag resolver for Confidence feature flags,
implementing the OpenFeature provider interface.
"""

from confidence_openfeature.version import __version__
from confidence_openfeature.provider import ConfidenceProvider
from confidence_openfeature.materialization import (
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
