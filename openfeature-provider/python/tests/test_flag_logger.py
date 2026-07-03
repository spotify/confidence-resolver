"""Tests for the flag logger module."""

import time
import threading
from unittest.mock import MagicMock, patch

from confidence.flag_logger import (
    FlagLogger,
    GrpcFlagLogger,
    NoOpFlagLogger,
    GRPC_TARGET,
)
from confidence.proto.confidence.flags.resolver.v1 import internal_api_pb2


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
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
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
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
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
            assert write_completed.is_set(), (
                "shutdown() returned before write completed"
            )


class TestGrpcFlagLoggerEmptyRequests:
    """Test that empty requests are skipped."""

    def test_empty_request_skipped(self) -> None:
        """Empty bytes should not be sent."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        stub_path = (
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
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
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
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
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
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


class TestGrpcFlagLoggerFailureStats:
    """Test windowed failure stats logging."""

    def test_no_log_when_all_succeed(self) -> None:
        """No warning should be logged when all writes succeed."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()
        mock_stub.ClientWriteFlagLogs = MagicMock(
            return_value=internal_api_pb2.WriteFlagLogsResponse()
        )

        stub_path = (
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            flag_logger = GrpcFlagLogger(
                client_secret="test-secret", channel=mock_channel
            )

            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            for _ in range(10):
                flag_logger.write(request_bytes)

            flag_logger.shutdown()

        with patch("confidence.flag_logger.logger") as mock_logger:
            mock_logger.warning.assert_not_called()

    def test_warns_on_failures_at_window_boundary(self) -> None:
        """Warning should be logged at the 10-attempt window when failures > 0."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        call_count = 0

        def side_effect(*args, **kwargs):
            nonlocal call_count
            call_count += 1
            if call_count <= 3:
                raise Exception("unavailable")
            return internal_api_pb2.WriteFlagLogsResponse()

        mock_stub.ClientWriteFlagLogs = side_effect

        stub_path = (
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with (
            patch(stub_path, return_value=mock_stub),
            patch("confidence.flag_logger.logger") as mock_logger,
        ):
            flag_logger = GrpcFlagLogger(
                client_secret="test-secret", channel=mock_channel
            )

            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            for _ in range(10):
                flag_logger.write(request_bytes)

            flag_logger.shutdown()

            mock_logger.warning.assert_called_once()
            call_args = mock_logger.warning.call_args
            assert "Flag log write failures" in call_args[0][0]

    def test_no_log_before_window_boundary(self) -> None:
        """No warning should be logged before reaching the 10-attempt window."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()
        mock_stub.ClientWriteFlagLogs = MagicMock(side_effect=Exception("unavailable"))

        stub_path = (
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with (
            patch(stub_path, return_value=mock_stub),
            patch("confidence.flag_logger.logger") as mock_logger,
        ):
            flag_logger = GrpcFlagLogger(
                client_secret="test-secret", channel=mock_channel
            )

            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            # Only 5 writes — below the 10-attempt window
            for _ in range(5):
                flag_logger.write(request_bytes)

            flag_logger.shutdown()

            mock_logger.warning.assert_not_called()


class TestGrpcFlagLoggerRetryExhausted:
    """Test behavior when all writes fail (simulating exhausted retries)."""

    def test_all_writes_fail_stats_correct(self) -> None:
        """Failure stats should report 10/10 failures when all writes fail."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()
        mock_stub.ClientWriteFlagLogs = MagicMock(side_effect=Exception("unavailable"))

        stub_path = (
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with (
            patch(stub_path, return_value=mock_stub),
            patch("confidence.flag_logger.logger") as mock_logger,
        ):
            flag_logger = GrpcFlagLogger(
                client_secret="test-secret", channel=mock_channel
            )

            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            for _ in range(10):
                flag_logger.write(request_bytes)

            flag_logger.shutdown()

            mock_logger.warning.assert_called_once()
            call_args = mock_logger.warning.call_args
            assert "Flag log write failures" in call_args[0][0]
            assert call_args[0][1] == 10


class TestGrpcFlagLoggerShutdownDuringFailures:
    """Test shutdown behavior when writes are failing."""

    def test_shutdown_completes_when_writes_fail(self) -> None:
        """shutdown() should complete promptly even when all writes fail."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()
        mock_stub.ClientWriteFlagLogs = MagicMock(side_effect=Exception("unavailable"))

        stub_path = (
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            flag_logger = GrpcFlagLogger(
                client_secret="test-secret", channel=mock_channel
            )

            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            for _ in range(5):
                flag_logger.write(request_bytes)

            start = time.monotonic()
            flag_logger.shutdown()
            elapsed = time.monotonic() - start

            assert elapsed < 5.0, f"shutdown() took {elapsed:.2f}s, expected < 5s"

    def test_shutdown_drains_slow_failing_writes(self) -> None:
        """shutdown() should wait for slow failing writes to complete."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()

        call_count = 0

        def slow_fail(*args, **kwargs):
            nonlocal call_count
            call_count += 1
            time.sleep(0.1)
            raise Exception("unavailable")

        mock_stub.ClientWriteFlagLogs = slow_fail

        stub_path = (
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            flag_logger = GrpcFlagLogger(
                client_secret="test-secret", channel=mock_channel
            )

            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            for _ in range(3):
                flag_logger.write(request_bytes)

            flag_logger.shutdown()

            assert call_count == 3, f"Expected 3 calls, got {call_count}"


class TestGrpcFlagLoggerCustomChannel:
    """Test behavior with custom vs default gRPC channels."""

    def test_custom_channel_used_directly(self) -> None:
        """Custom channel should be used as-is, not owned by the logger."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()
        mock_stub.ClientWriteFlagLogs = MagicMock(
            return_value=internal_api_pb2.WriteFlagLogsResponse()
        )

        stub_path = (
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub) as mock_stub_class:
            flag_logger = GrpcFlagLogger(
                client_secret="test-secret", channel=mock_channel
            )

            mock_stub_class.assert_called_once_with(mock_channel)
            assert not flag_logger._owns_channel

            flag_logger.shutdown()

            mock_channel.close.assert_not_called()

    def test_default_channel_has_retry_config(self) -> None:
        """Default channel should be created with retry service config."""
        with (
            patch("confidence.flag_logger.grpc.secure_channel") as mock_secure_channel,
            patch("confidence.flag_logger.grpc.ssl_channel_credentials") as mock_creds,
            patch(
                "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
            ),
        ):
            mock_secure_channel.return_value = MagicMock()
            mock_creds.return_value = MagicMock()

            flag_logger = GrpcFlagLogger(client_secret="test-secret")

            mock_secure_channel.assert_called_once()
            call_kwargs = mock_secure_channel.call_args
            options = call_kwargs.kwargs.get("options")
            assert options is not None, "Options should be passed to secure_channel"

            config_found = False
            for key, value in options:
                if key == "grpc.service_config":
                    import json

                    config = json.loads(value)
                    assert "methodConfig" in config
                    retry_policy = config["methodConfig"][0]["retryPolicy"]
                    assert retry_policy["maxAttempts"] == 3
                    assert "UNAVAILABLE" in retry_policy["retryableStatusCodes"]
                    config_found = True
                    break

            assert config_found, "Retry service config not found in channel options"

            assert flag_logger._owns_channel
            flag_logger.shutdown()

    def test_multiple_writes_with_custom_channel(self) -> None:
        """Multiple writes through custom channel should all complete."""
        mock_channel = MagicMock()
        mock_stub = MagicMock()
        mock_stub.ClientWriteFlagLogs = MagicMock(
            return_value=internal_api_pb2.WriteFlagLogsResponse()
        )

        stub_path = (
            "confidence.flag_logger.internal_api_pb2_grpc.InternalFlagLoggerServiceStub"
        )
        with patch(stub_path, return_value=mock_stub):
            flag_logger = GrpcFlagLogger(
                client_secret="test-secret", channel=mock_channel
            )

            request = internal_api_pb2.WriteFlagLogsRequest()
            request.flag_assigned.add()
            request_bytes = request.SerializeToString()

            for _ in range(5):
                flag_logger.write(request_bytes)

            flag_logger.shutdown()

            assert mock_stub.ClientWriteFlagLogs.call_count == 5


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
