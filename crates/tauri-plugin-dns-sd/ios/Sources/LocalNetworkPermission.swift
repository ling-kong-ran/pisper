import Network

func isLocalNetworkPermissionDenied(_ error: NWError) -> Bool {
    switch error {
    case .dns(let code):
        return code == -65570
    case .posix(let code):
        return code == .EACCES || code == .EPERM
    default:
        return false
    }
}
