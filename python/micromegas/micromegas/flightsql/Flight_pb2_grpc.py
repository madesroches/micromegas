# Generated by the gRPC Python protocol compiler plugin. DO NOT EDIT!
"""Client and server classes corresponding to protobuf-defined services."""
import grpc
import warnings

from . import Flight_pb2 as Flight__pb2

GRPC_GENERATED_VERSION = '1.68.1'
GRPC_VERSION = grpc.__version__
_version_not_supported = False

try:
    from grpc._utilities import first_version_is_lower
    _version_not_supported = first_version_is_lower(GRPC_VERSION, GRPC_GENERATED_VERSION)
except ImportError:
    _version_not_supported = True

if _version_not_supported:
    raise RuntimeError(
        f'The grpc package installed is at version {GRPC_VERSION},'
        + f' but the generated code in Flight_pb2_grpc.py depends on'
        + f' grpcio>={GRPC_GENERATED_VERSION}.'
        + f' Please upgrade your grpc module to grpcio>={GRPC_GENERATED_VERSION}'
        + f' or downgrade your generated code using grpcio-tools<={GRPC_VERSION}.'
    )


class FlightServiceStub(object):
    """
    A flight service is an endpoint for retrieving or storing Arrow data. A
    flight service can expose one or more predefined endpoints that can be
    accessed using the Arrow Flight Protocol. Additionally, a flight service
    can expose a set of actions that are available.
    """

    def __init__(self, channel):
        """Constructor.

        Args:
            channel: A grpc.Channel.
        """
        self.Handshake = channel.stream_stream(
                '/arrow.flight.protocol.FlightService/Handshake',
                request_serializer=Flight__pb2.HandshakeRequest.SerializeToString,
                response_deserializer=Flight__pb2.HandshakeResponse.FromString,
                _registered_method=True)
        self.ListFlights = channel.unary_stream(
                '/arrow.flight.protocol.FlightService/ListFlights',
                request_serializer=Flight__pb2.Criteria.SerializeToString,
                response_deserializer=Flight__pb2.FlightInfo.FromString,
                _registered_method=True)
        self.GetFlightInfo = channel.unary_unary(
                '/arrow.flight.protocol.FlightService/GetFlightInfo',
                request_serializer=Flight__pb2.FlightDescriptor.SerializeToString,
                response_deserializer=Flight__pb2.FlightInfo.FromString,
                _registered_method=True)
        self.PollFlightInfo = channel.unary_unary(
                '/arrow.flight.protocol.FlightService/PollFlightInfo',
                request_serializer=Flight__pb2.FlightDescriptor.SerializeToString,
                response_deserializer=Flight__pb2.PollInfo.FromString,
                _registered_method=True)
        self.GetSchema = channel.unary_unary(
                '/arrow.flight.protocol.FlightService/GetSchema',
                request_serializer=Flight__pb2.FlightDescriptor.SerializeToString,
                response_deserializer=Flight__pb2.SchemaResult.FromString,
                _registered_method=True)
        self.DoGet = channel.unary_stream(
                '/arrow.flight.protocol.FlightService/DoGet',
                request_serializer=Flight__pb2.Ticket.SerializeToString,
                response_deserializer=Flight__pb2.FlightData.FromString,
                _registered_method=True)
        self.DoPut = channel.stream_stream(
                '/arrow.flight.protocol.FlightService/DoPut',
                request_serializer=Flight__pb2.FlightData.SerializeToString,
                response_deserializer=Flight__pb2.PutResult.FromString,
                _registered_method=True)
        self.DoExchange = channel.stream_stream(
                '/arrow.flight.protocol.FlightService/DoExchange',
                request_serializer=Flight__pb2.FlightData.SerializeToString,
                response_deserializer=Flight__pb2.FlightData.FromString,
                _registered_method=True)
        self.DoAction = channel.unary_stream(
                '/arrow.flight.protocol.FlightService/DoAction',
                request_serializer=Flight__pb2.Action.SerializeToString,
                response_deserializer=Flight__pb2.Result.FromString,
                _registered_method=True)
        self.ListActions = channel.unary_stream(
                '/arrow.flight.protocol.FlightService/ListActions',
                request_serializer=Flight__pb2.Empty.SerializeToString,
                response_deserializer=Flight__pb2.ActionType.FromString,
                _registered_method=True)


class FlightServiceServicer(object):
    """
    A flight service is an endpoint for retrieving or storing Arrow data. A
    flight service can expose one or more predefined endpoints that can be
    accessed using the Arrow Flight Protocol. Additionally, a flight service
    can expose a set of actions that are available.
    """

    def Handshake(self, request_iterator, context):
        """
        Handshake between client and server. Depending on the server, the
        handshake may be required to determine the token that should be used for
        future operations. Both request and response are streams to allow multiple
        round-trips depending on auth mechanism.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')

    def ListFlights(self, request, context):
        """
        Get a list of available streams given a particular criteria. Most flight
        services will expose one or more streams that are readily available for
        retrieval. This api allows listing the streams available for
        consumption. A user can also provide a criteria. The criteria can limit
        the subset of streams that can be listed via this interface. Each flight
        service allows its own definition of how to consume criteria.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')

    def GetFlightInfo(self, request, context):
        """
        For a given FlightDescriptor, get information about how the flight can be
        consumed. This is a useful interface if the consumer of the interface
        already can identify the specific flight to consume. This interface can
        also allow a consumer to generate a flight stream through a specified
        descriptor. For example, a flight descriptor might be something that
        includes a SQL statement or a Pickled Python operation that will be
        executed. In those cases, the descriptor will not be previously available
        within the list of available streams provided by ListFlights but will be
        available for consumption for the duration defined by the specific flight
        service.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')

    def PollFlightInfo(self, request, context):
        """
        For a given FlightDescriptor, start a query and get information
        to poll its execution status. This is a useful interface if the
        query may be a long-running query. The first PollFlightInfo call
        should return as quickly as possible. (GetFlightInfo doesn't
        return until the query is complete.)

        A client can consume any available results before
        the query is completed. See PollInfo.info for details.

        A client can poll the updated query status by calling
        PollFlightInfo() with PollInfo.flight_descriptor. A server
        should not respond until the result would be different from last
        time. That way, the client can "long poll" for updates
        without constantly making requests. Clients can set a short timeout
        to avoid blocking calls if desired.

        A client can't use PollInfo.flight_descriptor after
        PollInfo.expiration_time passes. A server might not accept the
        retry descriptor anymore and the query may be cancelled.

        A client may use the CancelFlightInfo action with
        PollInfo.info to cancel the running query.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')

    def GetSchema(self, request, context):
        """
        For a given FlightDescriptor, get the Schema as described in Schema.fbs::Schema
        This is used when a consumer needs the Schema of flight stream. Similar to
        GetFlightInfo this interface may generate a new flight that was not previously
        available in ListFlights.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')

    def DoGet(self, request, context):
        """
        Retrieve a single stream associated with a particular descriptor
        associated with the referenced ticket. A Flight can be composed of one or
        more streams where each stream can be retrieved using a separate opaque
        ticket that the flight service uses for managing a collection of streams.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')

    def DoPut(self, request_iterator, context):
        """
        Push a stream to the flight service associated with a particular
        flight stream. This allows a client of a flight service to upload a stream
        of data. Depending on the particular flight service, a client consumer
        could be allowed to upload a single stream per descriptor or an unlimited
        number. In the latter, the service might implement a 'seal' action that
        can be applied to a descriptor once all streams are uploaded.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')

    def DoExchange(self, request_iterator, context):
        """
        Open a bidirectional data channel for a given descriptor. This
        allows clients to send and receive arbitrary Arrow data and
        application-specific metadata in a single logical stream. In
        contrast to DoGet/DoPut, this is more suited for clients
        offloading computation (rather than storage) to a Flight service.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')

    def DoAction(self, request, context):
        """
        Flight services can support an arbitrary number of simple actions in
        addition to the possible ListFlights, GetFlightInfo, DoGet, DoPut
        operations that are potentially available. DoAction allows a flight client
        to do a specific action against a flight service. An action includes
        opaque request and response objects that are specific to the type action
        being undertaken.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')

    def ListActions(self, request, context):
        """
        A flight service exposes all of the available action types that it has
        along with descriptions. This allows different flight consumers to
        understand the capabilities of the flight service.
        """
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details('Method not implemented!')
        raise NotImplementedError('Method not implemented!')


def add_FlightServiceServicer_to_server(servicer, server):
    rpc_method_handlers = {
            'Handshake': grpc.stream_stream_rpc_method_handler(
                    servicer.Handshake,
                    request_deserializer=Flight__pb2.HandshakeRequest.FromString,
                    response_serializer=Flight__pb2.HandshakeResponse.SerializeToString,
            ),
            'ListFlights': grpc.unary_stream_rpc_method_handler(
                    servicer.ListFlights,
                    request_deserializer=Flight__pb2.Criteria.FromString,
                    response_serializer=Flight__pb2.FlightInfo.SerializeToString,
            ),
            'GetFlightInfo': grpc.unary_unary_rpc_method_handler(
                    servicer.GetFlightInfo,
                    request_deserializer=Flight__pb2.FlightDescriptor.FromString,
                    response_serializer=Flight__pb2.FlightInfo.SerializeToString,
            ),
            'PollFlightInfo': grpc.unary_unary_rpc_method_handler(
                    servicer.PollFlightInfo,
                    request_deserializer=Flight__pb2.FlightDescriptor.FromString,
                    response_serializer=Flight__pb2.PollInfo.SerializeToString,
            ),
            'GetSchema': grpc.unary_unary_rpc_method_handler(
                    servicer.GetSchema,
                    request_deserializer=Flight__pb2.FlightDescriptor.FromString,
                    response_serializer=Flight__pb2.SchemaResult.SerializeToString,
            ),
            'DoGet': grpc.unary_stream_rpc_method_handler(
                    servicer.DoGet,
                    request_deserializer=Flight__pb2.Ticket.FromString,
                    response_serializer=Flight__pb2.FlightData.SerializeToString,
            ),
            'DoPut': grpc.stream_stream_rpc_method_handler(
                    servicer.DoPut,
                    request_deserializer=Flight__pb2.FlightData.FromString,
                    response_serializer=Flight__pb2.PutResult.SerializeToString,
            ),
            'DoExchange': grpc.stream_stream_rpc_method_handler(
                    servicer.DoExchange,
                    request_deserializer=Flight__pb2.FlightData.FromString,
                    response_serializer=Flight__pb2.FlightData.SerializeToString,
            ),
            'DoAction': grpc.unary_stream_rpc_method_handler(
                    servicer.DoAction,
                    request_deserializer=Flight__pb2.Action.FromString,
                    response_serializer=Flight__pb2.Result.SerializeToString,
            ),
            'ListActions': grpc.unary_stream_rpc_method_handler(
                    servicer.ListActions,
                    request_deserializer=Flight__pb2.Empty.FromString,
                    response_serializer=Flight__pb2.ActionType.SerializeToString,
            ),
    }
    generic_handler = grpc.method_handlers_generic_handler(
            'arrow.flight.protocol.FlightService', rpc_method_handlers)
    server.add_generic_rpc_handlers((generic_handler,))
    server.add_registered_method_handlers('arrow.flight.protocol.FlightService', rpc_method_handlers)


 # This class is part of an EXPERIMENTAL API.
class FlightService(object):
    """
    A flight service is an endpoint for retrieving or storing Arrow data. A
    flight service can expose one or more predefined endpoints that can be
    accessed using the Arrow Flight Protocol. Additionally, a flight service
    can expose a set of actions that are available.
    """

    @staticmethod
    def Handshake(request_iterator,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.stream_stream(
            request_iterator,
            target,
            '/arrow.flight.protocol.FlightService/Handshake',
            Flight__pb2.HandshakeRequest.SerializeToString,
            Flight__pb2.HandshakeResponse.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)

    @staticmethod
    def ListFlights(request,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.unary_stream(
            request,
            target,
            '/arrow.flight.protocol.FlightService/ListFlights',
            Flight__pb2.Criteria.SerializeToString,
            Flight__pb2.FlightInfo.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)

    @staticmethod
    def GetFlightInfo(request,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.unary_unary(
            request,
            target,
            '/arrow.flight.protocol.FlightService/GetFlightInfo',
            Flight__pb2.FlightDescriptor.SerializeToString,
            Flight__pb2.FlightInfo.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)

    @staticmethod
    def PollFlightInfo(request,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.unary_unary(
            request,
            target,
            '/arrow.flight.protocol.FlightService/PollFlightInfo',
            Flight__pb2.FlightDescriptor.SerializeToString,
            Flight__pb2.PollInfo.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)

    @staticmethod
    def GetSchema(request,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.unary_unary(
            request,
            target,
            '/arrow.flight.protocol.FlightService/GetSchema',
            Flight__pb2.FlightDescriptor.SerializeToString,
            Flight__pb2.SchemaResult.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)

    @staticmethod
    def DoGet(request,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.unary_stream(
            request,
            target,
            '/arrow.flight.protocol.FlightService/DoGet',
            Flight__pb2.Ticket.SerializeToString,
            Flight__pb2.FlightData.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)

    @staticmethod
    def DoPut(request_iterator,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.stream_stream(
            request_iterator,
            target,
            '/arrow.flight.protocol.FlightService/DoPut',
            Flight__pb2.FlightData.SerializeToString,
            Flight__pb2.PutResult.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)

    @staticmethod
    def DoExchange(request_iterator,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.stream_stream(
            request_iterator,
            target,
            '/arrow.flight.protocol.FlightService/DoExchange',
            Flight__pb2.FlightData.SerializeToString,
            Flight__pb2.FlightData.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)

    @staticmethod
    def DoAction(request,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.unary_stream(
            request,
            target,
            '/arrow.flight.protocol.FlightService/DoAction',
            Flight__pb2.Action.SerializeToString,
            Flight__pb2.Result.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)

    @staticmethod
    def ListActions(request,
            target,
            options=(),
            channel_credentials=None,
            call_credentials=None,
            insecure=False,
            compression=None,
            wait_for_ready=None,
            timeout=None,
            metadata=None):
        return grpc.experimental.unary_stream(
            request,
            target,
            '/arrow.flight.protocol.FlightService/ListActions',
            Flight__pb2.Empty.SerializeToString,
            Flight__pb2.ActionType.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True)