package confidence

import (
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
)

type FlagLogger interface {
	Write(request *resolverv1.WriteFlagLogsRequest)
	Shutdown()
}
