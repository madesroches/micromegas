package flightsql

import (
	"context"
	"crypto/x509"
	"fmt"
	"sync"

	"github.com/apache/arrow/go/v12/arrow/flight"
	"github.com/apache/arrow/go/v12/arrow/flight/flightsql"
	"github.com/apache/arrow/go/v12/arrow/ipc"
	"github.com/apache/arrow/go/v12/arrow/memory"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/metadata"
)

func newFlightSQLClient(cfg config) (*client, error) {
	dialOptions, err := grpcDialOptions(cfg)
	if err != nil {
		return nil, fmt.Errorf("grpc dial options: %s", err)
	}
	fsqlc, err := flightsql.NewClient(cfg.Addr, nil, nil, dialOptions...)
	if err != nil {
		return nil, err
	}
	return &client{fsqlc}, nil
}

func grpcDialOptions(cfg config) ([]grpc.DialOption, error) {
	transport := grpc.WithTransportCredentials(insecure.NewCredentials())
	if cfg.Secure {
		pool, err := x509.SystemCertPool()
		if err != nil {
			return nil, fmt.Errorf("x509: %s", err)
		}
		transport = grpc.WithTransportCredentials(credentials.NewClientTLSFromCert(pool, ""))
	}

	opts := []grpc.DialOption{
		transport,
	}

	return opts, nil
}

// client wraps a [flightsql.Client] client to extend its behavior.
//
// The API provided by flightsql.Client provides no access to gRPC headers for
// streaming operations such as DoGet. The methods implemented on this type aim
// to provide access to that information.
type client struct {
	*flightsql.Client
}

// FlightClient returns the underlying [flight.Client].
func (c *client) FlightClient() flight.Client {
	return c.Client.Client
}

// DoGetWithHeaderExtraction performs a normal DoGet, but wraps the stream in a
// mechanism that extracts headers when they become available. At least one
// record should be read from the *flightReader before the headers are
// available.
func (c *client) DoGetWithHeaderExtraction(ctx context.Context, in *flight.Ticket, opts ...grpc.CallOption) (*flightReader, error) {
	stream, err := c.FlightClient().DoGet(ctx, in, opts...)
	if err != nil {
		return nil, err
	}
	return newFlightReader(stream, c.Client.Alloc)
}

// flightReader wraps a [flight.Reader] to expose the headers captured when the
// first read occurs on the stream.
type flightReader struct {
	*flight.Reader
	extractor *headerExtractor
}

// newFlightReader returns a [flightReader].
func newFlightReader(stream flight.FlightService_DoGetClient, alloc memory.Allocator) (*flightReader, error) {
	extractor := &headerExtractor{stream: stream}
	reader, err := flight.NewRecordReader(extractor, ipc.WithAllocator(alloc))
	if err != nil {
		return nil, err
	}
	return &flightReader{
		Reader:    reader,
		extractor: extractor,
	}, nil
}

// Header returns the extracted headers if they exist.
func (s *flightReader) Header() (metadata.MD, error) {
	return s.extractor.Header()
}

// headerExtractor collects the stream's headers on the first call to
// [(*headerExtractor).Recv].
type headerExtractor struct {
	stream flight.FlightService_DoGetClient

	once   sync.Once
	header metadata.MD
	err    error
}

// Header returns the extracted headers if they exist.
func (s *headerExtractor) Header() (metadata.MD, error) {
	return s.header, s.err
}

// Recv reads from the stream. The first invocation will capture the headers.
func (s *headerExtractor) Recv() (*flight.FlightData, error) {
	data, err := s.stream.Recv()
	s.once.Do(func() {
		s.header, s.err = s.stream.Header()
	})
	return data, err
}
