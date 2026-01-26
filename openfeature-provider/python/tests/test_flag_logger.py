"""Tests for the flag logger module."""

import time
import threading
from unittest.mock import MagicMock, patch

from confidence_openfeature.flag_logger import (
    FlagLogger,
    GrpcFlagLogger,
    NoOpFlagLogger,
    GRPC_TARGET,
)
from confidence_openfeature.proto.confidence.flags.resolver.v1 import internal_api_pb2


class TestGrpcFlagLoggerProtocol:
    """Test that GrpcFlagLogger implements the FlagLogger protocol."""

    def test_implements_protocol(self) -> None:
        """GrpcFlagLogger should implement FlagLogger protocol."""
        mock_channel = MagicMock()
        logger = GrpcFlagLogger(client_secret="test-secret", channel=mock_channel)
        assert isinstance(logger, FlagLogger)
        logger.shutdown()


class TestGrpcFlagLoggerAsync:
    """Test async behavior of GrpcFlagLogger."""

    def test_write_async(self) -> None:
        """write() should return immediately (non-blocking)."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        # Make the stub method block for a while
        write_started = threading.Event()
        write_can_complete = threading.Event()

        def slow_write(*args, **kwargs):
            write_started.set()
            write_can_complete.wait(timeout=5.0)
            return internal_api_pb2.WriteFlagLogsResponse()

        mock_stub.ClientWriteFlagLogs = slow_write

        stub_path = (
            "confidence_openfeature.flag_logger."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            logger = GrpcFlagLogger(client_secret="test-secret", channel=mock_channel)

            # Create a valid request with data
            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            # write() should return immediately
            start_time = time.monotonic()
            logger.write(request_bytes)
            elapsed = time.monotonic() - start_time

            # Wait for the async write to start
            assert write_started.wait(timeout=2.0), "Async write did not start"

            # write() should have returned quickly (before the slow write completed)
            assert elapsed < 0.5, f"write() blocked for {elapsed:.2f}s, expected < 0.5s"

            # Allow the write to complete and shutdown
            write_can_complete.set()
            logger.shutdown()

    def test_shutdown_waits_for_pending(self) -> None:
        """shutdown() should wait for pending writes to complete."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        write_completed = threading.Event()

        def slow_write(*args, **kwargs):
            time.sleep(0.2)  # Simulate slow write
            write_completed.set()
            return internal_api_pb2.WriteFlagLogsResponse()

        mock_stub.ClientWriteFlagLogs = slow_write

        stub_path = (
            "confidence_openfeature.flag_logger."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            logger = GrpcFlagLogger(client_secret="test-secret", channel=mock_channel)

            # Create a valid request with data
            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            # Submit a write
            logger.write(request_bytes)

            # shutdown() should wait for the write to complete
            logger.shutdown()

            # After shutdown, the write should have completed
            assert (
                write_completed.is_set()
            ), "shutdown() returned before write completed"


class TestGrpcFlagLoggerEmptyRequests:
    """Test that empty requests are skipped."""

    def test_empty_request_skipped(self) -> None:
        """Empty bytes should not be sent."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        stub_path = (
            "confidence_openfeature.flag_logger."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            logger = GrpcFlagLogger(client_secret="test-secret", channel=mock_channel)

            # Write empty bytes
            logger.write(b"")

            # Wait a bit and shutdown
            time.sleep(0.1)
            logger.shutdown()

            # Should not have called the stub
            mock_stub.ClientWriteFlagLogs.assert_not_called()

    def test_empty_fields_skipped(self) -> None:
        """Request with no data should not be sent."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        stub_path = (
            "confidence_openfeature.flag_logger."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            logger = GrpcFlagLogger(client_secret="test-secret", channel=mock_channel)

            # Create an empty request (no data fields)
            request = internal_api_pb2.WriteFlagLogsRequest()
            request_bytes = request.SerializeToString()

            logger.write(request_bytes)

            # Wait a bit and shutdown
            time.sleep(0.1)
            logger.shutdown()

            # Should not have called the stub
            mock_stub.ClientWriteFlagLogs.assert_not_called()


class TestGrpcFlagLoggerAuthorization:
    """Test authorization header handling."""

    def test_authorization_header(self) -> None:
        """ClientSecret header should be added to gRPC calls."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()
        mock_stub.ClientWriteFlagLogs = MagicMock(
            return_value=internal_api_pb2.WriteFlagLogsResponse()
        )

        client_secret = "my-secret-key"

        stub_path = (
            "confidence_openfeature.flag_logger."
            "internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            logger = GrpcFlagLogger(client_secret=client_secret, channel=mock_channel)

            # Create a valid request with data
            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            logger.write(request_bytes)
            logger.shutdown()

            # Check that the stub was called with the correct metadata
            mock_stub.ClientWriteFlagLogs.assert_called_once()
            call_kwargs = mock_stub.ClientWriteFlagLogs.call_args

            # Check metadata contains authorization header
            metadata = call_kwargs.kwargs.get("metadata") or call_kwargs[1].get(
                "metadata"
            )
            assert metadata is not None, "metadata should be provided"

            auth_header = None
            for key, value in metadata:
                if key == "authorization":
                    auth_header = value
                    break

            assert auth_header == f"ClientSecret {client_secret}"


class TestGrpcFlagLoggerGrpcTarget:
    """Test gRPC target configuration."""

    def test_grpc_target_constant(self) -> None:
        """GRPC_TARGET should be set to the expected value."""
        assert GRPC_TARGET == "edge-grpc.spotify.com:443"


class TestNoOpFlagLogger:
    """Test NoOpFlagLogger behavior."""

    def test_noop_logger(self) -> None:
        """NoOpFlagLogger should do nothing."""
        logger = NoOpFlagLogger()

        # These should not raise
        logger.write(b"some data")
        logger.write(b"")
        logger.shutdown()

        # Verify it implements the protocol
        assert isinstance(logger, FlagLogger)

    def test_noop_logger_multiple_writes(self) -> None:
        """NoOpFlagLogger should handle multiple writes without issues."""
        logger = NoOpFlagLogger()

        for i in range(100):
            logger.write(f"data {i}".encode())

        logger.shutdown()
