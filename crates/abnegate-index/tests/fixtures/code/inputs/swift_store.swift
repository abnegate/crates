import Foundation
import XCTest

/// A bookmark saved by the reader.
struct Bookmark: Codable, Equatable {
    let id: UUID
    var title: String
    var url: URL
}

protocol BookmarkStoring {
    func save(_ bookmark: Bookmark) throws
    func all() -> [Bookmark]
}

final class BookmarkStore: BookmarkStoring {
    private var bookmarks: [UUID: Bookmark] = [:]
    private let queue = DispatchQueue(label: "bookmarks")

    func save(_ bookmark: Bookmark) throws {
        queue.sync {
            bookmarks[bookmark.id] = bookmark
        }
    }

    func all() -> [Bookmark] {
        queue.sync {
            bookmarks.values.sorted { $0.title < $1.title }
        }
    }
}

extension Bookmark {
    var host: String {
        url.host ?? "unknown"
    }
}

enum SortOrder {
    case title
    case recent
}

final class BookmarkStoreTests: XCTestCase {
    func testSaveThenAll() throws {
        let store = BookmarkStore()
        try store.save(Bookmark(id: UUID(), title: "Docs", url: URL(string: "https://example.com")!))
        XCTAssertEqual(store.all().count, 1)
    }
}
