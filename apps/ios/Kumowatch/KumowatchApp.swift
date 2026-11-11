import SwiftUI

@main
struct KumowatchApp: App {
    @UIApplicationDelegateAdaptor(PushDelegate.self) private var pushDelegate
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .onOpenURL { url in Task { await model.handle(url: url) } }
                .onAppear {
                    pushDelegate.model = model
                    if model.relay.isConfigured { model.registerForPush() }
                }
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active { model.onForeground() }
                }
        }
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        switch model.screen {
        case .signIn: SignInView()
        case .pickSpace: NavigationStack { SpacesView() }
        case .chats: MainTabs()
        }
    }
}

struct MainTabs: View {
    @Environment(AppModel.self) private var model
    @State private var showCall = false
    @State private var password = ""
    @State private var tab = 0

    var body: some View {
        TabView(selection: $tab) {
            ChatListView()
                .tabItem { Label("Chat", systemImage: "bubble.left.and.bubble.right") }
                .tag(0)
            OfficeView(showCall: $showCall)
                .tabItem { Label("Office", systemImage: "building.2") }
                .tag(1)
        }
        .onChange(of: model.openRequest) { _, url in
            if url != nil { tab = 0 }
        }
        .fullScreenCover(isPresented: $showCall) { CallView() }
        .alert("Room password", isPresented: Binding(
            get: { model.passwordPrompt != nil },
            set: { if !$0 && model.passwordPrompt != nil { model.submitPassword(nil) } }
        )) {
            SecureField("Password", text: $password)
            Button("Join") {
                model.submitPassword(password)
                password = ""
            }
            Button("Cancel", role: .cancel) { model.submitPassword(nil) }
        }
        .alert(model.notice ?? "", isPresented: Binding(
            get: { model.notice != nil },
            set: { if !$0 { model.notice = nil } }
        )) {
            Button("OK", role: .cancel) {}
        }
    }
}
