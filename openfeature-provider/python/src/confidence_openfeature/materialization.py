"""Materialization store module for Confidence OpenFeature provider.

This module provides storage abstractions for materialization data used in
flag resolution.

Materializations support two key use cases:
1. Sticky Assignments: Maintain consistent variant assignments across evaluations
   even when targeting attributes change.
2. Custom Targeting via Materialized Segments: Precomputed sets of identifiers from
   datasets that should be targeted.
"""

from dataclasses import dataclass
from typing import List, Optional, Protocol, Union

import grpc

from confidence_openfeature.proto.confidence.flags.resolver.v1 import (
    internal_api_pb2,
    internal_api_pb2_grpc,
)


@dataclass
class VariantReadOp:
    """Query operation to retrieve the variant assignment for a unit and rule.

    Used for sticky assignments to fetch the previously assigned variant.

    Attributes:
        unit: The entity identifier (user ID, session ID, etc.)
        materialization: The materialization context identifier
        rule: The targeting rule identifier
    """

    unit: str
    materialization: str
    rule: str


@dataclass
class VariantReadResult:
    """Result containing the variant assignment for a unit and rule.

    Used for sticky assignments - returns the previously assigned variant for a
    unit and targeting rule combination.

    Attributes:
        unit: The entity identifier (user ID, session ID, etc.)
        materialization: The materialization context identifier
        rule: The targeting rule identifier
        variant: The assigned variant name, or None if no assignment exists
    """

    unit: str
    materialization: str
    rule: str
    variant: Optional[str] = None


@dataclass
class VariantWriteOp:
    """A variant assignment write operation.

    Used to store sticky variant assignments, recording which variant a unit
    (user, session, etc.) should receive for a specific targeting rule.

    Attributes:
        unit: The entity identifier (user ID, session ID, etc.)
        materialization: The materialization context identifier
        rule: The targeting rule identifier
        variant: The assigned variant name
    """

    unit: str
    materialization: str
    rule: str
    variant: str


@dataclass
class InclusionReadOp:
    """Query operation to check if a unit is included in a materialized segment.

    Used for custom targeting via materialized segments - checks if a unit
    is part of a precomputed set of identifiers.

    Attributes:
        unit: The entity identifier (user ID, session ID, etc.)
        materialization: The materialization context identifier
    """

    unit: str
    materialization: str


@dataclass
class InclusionReadResult:
    """Result containing whether a unit is included in a materialized segment.

    Attributes:
        unit: The entity identifier (user ID, session ID, etc.)
        materialization: The materialization context identifier
        included: Whether the unit is included in the segment
    """

    unit: str
    materialization: str
    included: bool


class MaterializationNotSupportedError(Exception):
    """Raised when a MaterializationStore doesn't support the requested operation.

    This triggers the provider to fall back to remote gRPC resolution via the
    Confidence service, which manages materializations server-side.
    """

    def __init__(
        self, message: str = "materialization operation not supported"
    ) -> None:
        super().__init__(message)
        self.message = message


# Type aliases for read operations and results
ReadOp = Union[VariantReadOp, InclusionReadOp]
ReadResult = Union[VariantReadResult, InclusionReadResult]


class MaterializationStore(Protocol):
    """Storage abstraction for materialization data used in flag resolution.

    Materializations support two key use cases:
    1. Sticky Assignments: Maintain consistent variant assignments across
       evaluations even when targeting attributes change.
    2. Custom Targeting via Materialized Segments: Precomputed sets of
       identifiers that should be targeted.

    Thread Safety: Implementations must be thread-safe as they may be called
    concurrently from multiple threads resolving flags in parallel.
    """

    def read(self, ops: List[ReadOp]) -> List[ReadResult]:
        """Perform a batch read of materialization data.

        Args:
            ops: The list of read operations to perform
                (VariantReadOp or InclusionReadOp)

        Returns:
            The read results (VariantReadResult or InclusionReadResult)

        Raises:
            MaterializationNotSupportedError: If the store doesn't support reads
        """
        ...

    def write(self, ops: List[VariantWriteOp]) -> None:
        """Perform a batch write of materialization data.

        Args:
            ops: The list of write operations to perform

        Raises:
            MaterializationNotSupportedError: If the store doesn't support writes
        """
        ...


class UnsupportedMaterializationStore:
    """A MaterializationStore that always raises MaterializationNotSupportedError.

    This is the default store used by the provider to trigger fallback to remote
    gRPC resolution when materializations are needed.
    """

    def read(self, ops: List[ReadOp]) -> List[ReadResult]:
        """Read always raises MaterializationNotSupportedError.

        Args:
            ops: The list of read operations to perform

        Raises:
            MaterializationNotSupportedError: Always raised
        """
        raise MaterializationNotSupportedError("materialization read not supported")

    def write(self, ops: List[VariantWriteOp]) -> None:
        """Write always raises MaterializationNotSupportedError.

        Args:
            ops: The list of write operations to perform

        Raises:
            MaterializationNotSupportedError: Always raised
        """
        raise MaterializationNotSupportedError("materialization write not supported")


class RemoteMaterializationStore:
    """A MaterializationStore that stores data remotely via gRPC.

    This implementation is useful when you want the Confidence service to manage
    materialization storage server-side rather than maintaining local state.
    """

    GRPC_TARGET = "edge-grpc.spotify.com:443"

    def __init__(
        self,
        client_secret: str,
        channel: Optional[grpc.Channel] = None,
    ) -> None:
        """Initialize the remote materialization store.

        Args:
            client_secret: The client secret for authentication
            channel: Optional gRPC channel. If not provided, a secure channel
                    will be created to GRPC_TARGET.
        """
        self._client_secret = client_secret
        if channel is None:
            channel = grpc.secure_channel(
                self.GRPC_TARGET,
                grpc.ssl_channel_credentials(),
            )
        self._channel = channel
        self._stub = internal_api_pb2_grpc.InternalFlagLoggerServiceStub(
            channel
        )  # type: ignore[no-untyped-call]

    def _get_metadata(self) -> List[tuple[str, str]]:
        """Get the authorization metadata for gRPC calls."""
        return [("authorization", f"ClientSecret {self._client_secret}")]

    def read(self, ops: List[ReadOp]) -> List[ReadResult]:
        """Perform a batch read of materialization data from the remote service.

        Args:
            ops: The list of read operations to perform
                (VariantReadOp or InclusionReadOp)

        Returns:
            The read results (VariantReadResult or InclusionReadResult)
        """
        if not ops:
            return []

        # Convert to proto format
        proto_ops = []
        for op in ops:
            proto_op = internal_api_pb2.ReadOp()  # type: ignore[attr-defined]
            if isinstance(op, VariantReadOp):
                proto_op.variant_read_op.CopyFrom(
                    internal_api_pb2.VariantReadOp(  # type: ignore[attr-defined]
                        unit=op.unit,
                        materialization=op.materialization,
                        rule=op.rule,
                    )
                )
            elif isinstance(op, InclusionReadOp):
                proto_op.inclusion_read_op.CopyFrom(
                    internal_api_pb2.InclusionReadOp(  # type: ignore[attr-defined]
                        unit=op.unit,
                        materialization=op.materialization,
                    )
                )
            proto_ops.append(proto_op)

        # Build request
        request = internal_api_pb2.ReadOperationsRequest(  # type: ignore[attr-defined]
            ops=proto_ops
        )

        # Call gRPC service
        response = self._stub.ReadMaterializedOperations(
            request,
            metadata=self._get_metadata(),
            timeout=30.0,
        )

        # Convert results
        results: List[ReadResult] = []
        for proto_result in response.results:
            if proto_result.HasField("variant_result"):
                variant_data = proto_result.variant_result
                results.append(
                    VariantReadResult(
                        unit=variant_data.unit,
                        materialization=variant_data.materialization,
                        rule=variant_data.rule,
                        variant=variant_data.variant if variant_data.variant else None,
                    )
                )
            elif proto_result.HasField("inclusion_result"):
                inclusion_data = proto_result.inclusion_result
                results.append(
                    InclusionReadResult(
                        unit=inclusion_data.unit,
                        materialization=inclusion_data.materialization,
                        included=inclusion_data.is_included,
                    )
                )
        return results

    def write(self, ops: List[VariantWriteOp]) -> None:
        """Perform a batch write of materialization data to the remote service.

        Args:
            ops: The list of write operations to perform
        """
        if not ops:
            return

        # Convert to proto format
        proto_ops = []
        for op in ops:
            proto_ops.append(
                internal_api_pb2.VariantData(  # type: ignore[attr-defined]
                    unit=op.unit,
                    materialization=op.materialization,
                    rule=op.rule,
                    variant=op.variant,
                )
            )

        # Build request
        request = internal_api_pb2.WriteOperationsRequest(  # type: ignore[attr-defined]
            store_variant_op=proto_ops
        )

        # Call gRPC service
        self._stub.WriteMaterializedOperations(
            request,
            metadata=self._get_metadata(),
            timeout=30.0,
        )
