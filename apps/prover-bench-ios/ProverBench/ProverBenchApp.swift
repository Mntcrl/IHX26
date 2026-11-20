import SwiftUI

@main
struct ProverBenchApp: App {
    @StateObject private var runner = BenchRunner()

    var body: some Scene {
        WindowGroup {
            ContentView(runner: runner)
                .task { runner.autorunIfRequested() }
        }
    }
}

struct ContentView: View {
    @ObservedObject var runner: BenchRunner

    var body: some View {
        NavigationStack {
            List {
                Section("Circuits (Documents/galaxy-proverbench/ or bundle)") {
                    if runner.circuits.isEmpty {
                        Text("No artifacts found. Push <name>/<name>.graph.bin, <name>.zkey, input.json into Documents/galaxy-proverbench/.")
                            .font(.footnote)
                    }
                    ForEach(runner.circuits, id: \.name) { c in
                        Button {
                            runner.run(names: [c.name], repeats: 1)
                        } label: {
                            VStack(alignment: .leading) {
                                Text(c.name).font(.headline)
                                Text(String(format: "zkey %.1f MB · graph %.1f MB", c.zkeyMB, c.graphMB))
                                    .font(.caption)
                            }
                        }
                        .disabled(runner.busy)
                    }
                }
                Section("Status") {
                    Text(runner.status).font(.system(.footnote, design: .monospaced))
                    if runner.busy {
                        Button("Cancel", role: .destructive) { runner.cancel() }
                    }
                }
                Section("Log") {
                    ForEach(Array(runner.log.enumerated()), id: \.offset) { _, line in
                        Text(line).font(.system(.caption2, design: .monospaced))
                    }
                }
            }
            .navigationTitle("Galaxy prover bench")
            .toolbar { Button("Rescan") { runner.scan() } }
        }
    }
}
