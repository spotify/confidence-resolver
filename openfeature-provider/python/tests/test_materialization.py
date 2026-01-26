"""Tests for the materialization store module."""

from unittest.mock import MagicMock, patch

import pytest

from confidence_openfeature.materialization import (
    MaterializationNotSupportedError,
    RemoteMaterializationStore,
    UnsupportedMaterializationStore,
    VariantReadOp,
    VariantReadResult,
    VariantWriteOp,
)


class TestDataClasses:
    """Tests for data classes."""

    def test_variant_read_op_creation(self) -> None:
        """Test VariantReadOp can be created with required fields."""
        op = VariantReadOp(
            unit="user-123",
            materialization="mat-456",
            rule="rule-789",
        )
        assert op.unit == "user-123"
        assert op.materialization == "mat-456"
        assert op.rule == "rule-789"

    def test_variant_read_result_creation(self) -> None:
        """Test VariantReadResult can be created with required fields."""
        result = VariantReadResult(
            unit="user-123",
            materialization="mat-456",
            rule="rule-789",
            variant="control",
        )
        assert result.unit == "user-123"
        assert result.materialization == "mat-456"
        assert result.rule == "rule-789"
        assert result.variant == "control"

    def test_variant_read_result_with_none_variant(self) -> None:
        """Test VariantReadResult can be created with None variant."""
        result = VariantReadResult(
            unit="user-123",
            materialization="mat-456",
            rule="rule-789",
            variant=None,
        )
        assert result.variant is None

    def test_variant_write_op_creation(self) -> None:
        """Test VariantWriteOp can be created with required fields."""
        op = VariantWriteOp(
            unit="user-123",
            materialization="mat-456",
            rule="rule-789",
            variant="treatment",
        )
        assert op.unit == "user-123"
        assert op.materialization == "mat-456"
        assert op.rule == "rule-789"
        assert op.variant == "treatment"


class TestUnsupportedMaterializationStore:
    """Tests for UnsupportedMaterializationStore."""

    def test_read_raises_materialization_not_supported_error(self) -> None:
        """Test that read raises MaterializationNotSupportedError."""
        store = UnsupportedMaterializationStore()
        ops = [VariantReadOp(unit="user-1", materialization="mat-1", rule="rule-1")]

        with pytest.raises(MaterializationNotSupportedError):
            store.read(ops)

    def test_write_raises_materialization_not_supported_error(self) -> None:
        """Test that write raises MaterializationNotSupportedError."""
        store = UnsupportedMaterializationStore()
        ops = [
            VariantWriteOp(
                unit="user-1", materialization="mat-1", rule="rule-1", variant="v1"
            )
        ]

        with pytest.raises(MaterializationNotSupportedError):
            store.write(ops)


class TestRemoteMaterializationStore:
    """Tests for RemoteMaterializationStore."""

    def test_remote_store_read(self) -> None:
        """RemoteMaterializationStore.read calls gRPC and returns results."""
        # Create mock channel and stub
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        # Setup mock response
        from confidence_openfeature.proto.confidence.flags.resolver.v1 import (
            internal_api_pb2,
        )

        mock_result = internal_api_pb2.ReadResult()
        mock_result.variant_result.CopyFrom(
            internal_api_pb2.VariantData(
                unit="user-123",
                materialization="mat-456",
                rule="rule-789",
                variant="control",
            )
        )
        mock_response = internal_api_pb2.ReadOperationsResult(results=[mock_result])
        mock_stub.ReadMaterializedOperations.return_value = mock_response

        # Create store with mock
        stub_path = (
            "confidence_openfeature.materialization."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            store = RemoteMaterializationStore(
                client_secret="test-secret",
                channel=mock_channel,
            )

            ops = [
                VariantReadOp(
                    unit="user-123", materialization="mat-456", rule="rule-789"
                )
            ]
            results = store.read(ops)

        # Verify results
        assert len(results) == 1
        assert results[0].unit == "user-123"
        assert results[0].materialization == "mat-456"
        assert results[0].rule == "rule-789"
        assert results[0].variant == "control"

        # Verify gRPC was called correctly
        mock_stub.ReadMaterializedOperations.assert_called_once()
        call_args = mock_stub.ReadMaterializedOperations.call_args
        request = call_args[0][0]
        assert len(request.ops) == 1
        assert request.ops[0].variant_read_op.unit == "user-123"
        assert request.ops[0].variant_read_op.materialization == "mat-456"
        assert request.ops[0].variant_read_op.rule == "rule-789"

        # Verify metadata contains authorization
        metadata = call_args[1]["metadata"]
        assert ("authorization", "ClientSecret test-secret") in metadata

    def test_remote_store_read_empty_ops(self) -> None:
        """RemoteMaterializationStore.read with empty ops returns empty list."""
        mock_channel = MagicMock()

        stub_path = (
            "confidence_openfeature.materialization."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path):
            store = RemoteMaterializationStore(
                client_secret="test-secret",
                channel=mock_channel,
            )
            results = store.read([])

        assert results == []

    def test_remote_store_write(self) -> None:
        """RemoteMaterializationStore.write calls gRPC correctly."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        # Setup mock response
        from confidence_openfeature.proto.confidence.flags.resolver.v1 import (
            internal_api_pb2,
        )

        mock_stub.WriteMaterializedOperations.return_value = (
            internal_api_pb2.WriteOperationsResult()
        )

        stub_path = (
            "confidence_openfeature.materialization."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            store = RemoteMaterializationStore(
                client_secret="test-secret",
                channel=mock_channel,
            )

            ops = [
                VariantWriteOp(
                    unit="user-123",
                    materialization="mat-456",
                    rule="rule-789",
                    variant="treatment",
                )
            ]
            store.write(ops)

        # Verify gRPC was called correctly
        mock_stub.WriteMaterializedOperations.assert_called_once()
        call_args = mock_stub.WriteMaterializedOperations.call_args
        request = call_args[0][0]
        assert len(request.store_variant_op) == 1
        assert request.store_variant_op[0].unit == "user-123"
        assert request.store_variant_op[0].materialization == "mat-456"
        assert request.store_variant_op[0].rule == "rule-789"
        assert request.store_variant_op[0].variant == "treatment"

        # Verify metadata contains authorization
        metadata = call_args[1]["metadata"]
        assert ("authorization", "ClientSecret test-secret") in metadata

    def test_remote_store_write_empty_ops(self) -> None:
        """RemoteMaterializationStore.write with empty ops does nothing."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        stub_path = (
            "confidence_openfeature.materialization."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            store = RemoteMaterializationStore(
                client_secret="test-secret",
                channel=mock_channel,
            )
            store.write([])

        # Verify gRPC was NOT called
        mock_stub.WriteMaterializedOperations.assert_not_called()

    def test_remote_store_grpc_target_constant(self) -> None:
        """RemoteMaterializationStore has correct GRPC_TARGET constant."""
        expected = "edge-grpc.spotify.com:443"
        assert RemoteMaterializationStore.GRPC_TARGET == expected


class TestMaterializationStoreProtocol:
    """Tests for MaterializationStore protocol compliance."""

    def test_unsupported_store_is_materialization_store(self) -> None:
        """UnsupportedMaterializationStore implements MaterializationStore."""
        store = UnsupportedMaterializationStore()
        # Check it has the required methods
        assert hasattr(store, "read")
        assert hasattr(store, "write")
        assert callable(store.read)
        assert callable(store.write)

    def test_remote_store_is_materialization_store(self) -> None:
        """RemoteMaterializationStore implements MaterializationStore."""
        mock_channel = MagicMock()
        stub_path = (
            "confidence_openfeature.materialization."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path):
            store = RemoteMaterializationStore(
                client_secret="test-secret",
                channel=mock_channel,
            )
        # Check it has the required methods
        assert hasattr(store, "read")
        assert hasattr(store, "write")
        assert callable(store.read)
        assert callable(store.write)
