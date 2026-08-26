import Network
import XCTest
@testable import tauri_plugin_dns_sd

final class DnsSdPluginTests: XCTestCase {
    func testRecognizesLocalNetworkPolicyDenials() {
        XCTAssertTrue(isLocalNetworkPermissionDenied(.dns(-65570)))
        XCTAssertTrue(isLocalNetworkPermissionDenied(.posix(.EACCES)))
        XCTAssertTrue(isLocalNetworkPermissionDenied(.posix(.EPERM)))
    }

    func testDoesNotMisclassifyOtherBrowseFailures() {
        XCTAssertFalse(isLocalNetworkPermissionDenied(.dns(-65568)))
        XCTAssertFalse(isLocalNetworkPermissionDenied(.posix(.ECONNREFUSED)))
        XCTAssertFalse(isLocalNetworkPermissionDenied(.posix(.ETIMEDOUT)))
    }
}
