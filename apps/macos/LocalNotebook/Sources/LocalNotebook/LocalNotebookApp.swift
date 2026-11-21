import SwiftUI

@main
struct LocalNotebookApp: App {
    @StateObject private var state = AppState()

    var body: some Scene {
        WindowGroup {
            MainView()
                .environmentObject(state)
                .frame(minWidth: 1080, minHeight: 720)
                .onAppear { state.start() }
        }
    }
}
