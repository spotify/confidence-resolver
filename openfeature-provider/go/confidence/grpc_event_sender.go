package confidence

import (
	"context"
	"log/slog"
	"time"

	events "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/events"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/metadata"
	"google.golang.org/protobuf/types/known/timestamppb"
)

// GrpcEventSender sends events to the Confidence events backend via gRPC.
type GrpcEventSender struct {
	conn   *grpc.ClientConn
	client events.EventsServiceClient
	logger *slog.Logger
}

var _ EventSender = (*GrpcEventSender)(nil)

// NewGrpcEventSender creates a new gRPC event sender connecting to the given target.
func NewGrpcEventSender(target string, logger *slog.Logger, opts ...grpc.DialOption) (*GrpcEventSender, error) {
	if len(opts) == 0 {
		opts = []grpc.DialOption{grpc.WithTransportCredentials(insecure.NewCredentials())}
	}
	conn, err := grpc.NewClient(target, opts...)
	if err != nil {
		return nil, err
	}
	return &GrpcEventSender{
		conn:   conn,
		client: events.NewEventsServiceClient(conn),
		logger: logger,
	}, nil
}

func (s *GrpcEventSender) Send(response *wasm.FlushEventsResponse, clientSecret string) {
	req := &events.PublishEventsRequest{
		ClientSecret: clientSecret,
		SendTime:     timestamppb.Now(),
	}
	for _, e := range response.Events {
		req.Events = append(req.Events, &events.Event{
			EventDefinition: e.EventDefinition,
			Payload:         e.Payload,
			EventTime:       e.EventTime,
		})
	}

	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		md := metadata.Pairs("authorization", "ClientSecret "+clientSecret)
		ctx = metadata.NewOutgoingContext(ctx, md)
		if _, err := s.client.PublishEvents(ctx, req); err != nil {
			s.logger.Error("Failed to publish events", "error", err)
		} else {
			s.logger.Debug("Successfully published events", "count", len(req.Events))
		}
	}()
}

func (s *GrpcEventSender) Shutdown() {
	if s.conn != nil {
		s.conn.Close()
	}
}
