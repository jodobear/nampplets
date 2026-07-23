import SwiftUI
import NMPNativeRuntimeApple

public struct ContentView: View {
    @State private var selection = "Home"
    @State private var activity = "Loading trusted shell"
    @State private var artifact: NappletArtifact?

    public init() {}

    public var body: some View {
        NavigationSplitView {
            List(selection: $selection) {
                Label("Home", systemImage: "house").tag("Home")
                Label("Messages", systemImage: "bubble.left.and.bubble.right").tag("Messages")
                Label("Groups", systemImage: "person.3").tag("Groups")
                Label("Streams", systemImage: "play.rectangle").tag("Streams")
                Label("Tools", systemImage: "wrench.and.screwdriver").tag("Tools")
            }
            .navigationTitle("Workbench")
            .navigationSplitViewColumnWidth(min: 160, ideal: 190)
        } detail: {
            VStack(spacing: 0) {
                HSplitView {
                    GroupBox("Feed slot · bundled napplet") {
                        nappletSurface
                    }
                    .frame(minWidth: 520)

                    GroupBox("Detail slot") {
                        ContentUnavailableView(
                            "No selection",
                            systemImage: "doc.text.magnifyingglass",
                            description: Text("Events and profiles open here.")
                        )
                    }
                    .frame(minWidth: 280, idealWidth: 340)
                }
                .padding(12)

                GroupBox("Composer slot") {
                    HStack {
                        Image(systemName: "square.and.pencil")
                        Text("Native, legacy napplet, or surface renderer")
                            .foregroundStyle(.secondary)
                        Spacer()
                    }
                    .padding(8)
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 12)

                HStack(spacing: 8) {
                    Image(systemName: activitySymbol)
                        .foregroundStyle(activityColor)
                    Text(activity)
                    Spacer()
                    Text("Direct napplet network denied · ephemeral WebKit store")
                        .foregroundStyle(.secondary)
                }
                .font(.caption)
                .padding(.horizontal, 16)
                .frame(height: 34)
                .background(.bar)
                .accessibilityIdentifier("runtime-activity")
            }
            .navigationTitle(selection)
        }
        .toolbar {
            ToolbarItemGroup {
                Button("Account", systemImage: "person.crop.circle") {}
                Button("Search", systemImage: "magnifyingglass") {}
                Button("Install", systemImage: "shippingbox") {}
                Button("Activity", systemImage: "waveform.path.ecg") {}
                Button("Settings", systemImage: "gearshape") {}
            }
        }
        .task {
            do {
                let fixture = try GoodMorningFixture.load()
                let storageRoot = try runtimeStorageRoot()
                artifact = try await Task.detached {
                    try fixture.open(storageRoot: storageRoot)
                }.value
                activity = "Signed exact-build session ready"
            } catch {
                activity = "Refused: \(error.localizedDescription)"
            }
        }
        .frame(minWidth: 980, minHeight: 680)
    }

    @ViewBuilder
    private var nappletSurface: some View {
        if let artifact {
            TrustedNappletView(artifact: artifact) { event in
                switch event {
                case .loading:
                    activity = "Loading trusted shell"
                case .mounted:
                    activity = "Signed Good Morning napplet mounted"
                case .request(let type):
                    activity = "Mapped \(type) from napplet window"
                case .refused(let reason):
                    activity = "Refused: \(reason)"
                case .crashed:
                    activity = "Napplet WebView crashed"
                }
            }
            .accessibilityIdentifier("bundled-napplet")
        } else {
            ProgressView("Loading verified artifact…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var activitySymbol: String {
        activity.hasPrefix("Refused") || activity.contains("crashed")
            ? "exclamationmark.triangle.fill"
            : "checkmark.shield.fill"
    }

    private var activityColor: Color {
        activity.hasPrefix("Refused") || activity.contains("crashed") ? .orange : .green
    }

    private func runtimeStorageRoot() throws -> URL {
        let base = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        return base
            .appendingPathComponent(
                "io.f7z.nmp.native-runtime.workbench",
                isDirectory: true
            )
            .appendingPathComponent("runtime", isDirectory: true)
    }
}
