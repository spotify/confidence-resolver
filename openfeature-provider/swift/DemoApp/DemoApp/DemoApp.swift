// NOTE(unit-local experiment): single-screen SwiftUI iOS app that exercises
// `ConfidenceLocalProvider` against the local `unit-local-server`. Lives in
// the same branch as the rest of the POC so reviewers don't need to switch
// repos to see end-to-end behaviour.
//
// To run:
//   1. From the repo root: `cargo run -p unit-local-server`
//      (default listens on http://127.0.0.1:8787)
//   2. In Xcode, open `DemoApp.xcodeproj` and run the iPhone simulator scheme.
//      Optional env vars in the run scheme:
//        UNIT_LOCAL_SERVER       (default http://127.0.0.1:8787)
//        CLIENT_SECRET           (default the fixture's confidence-demo-june secret)
//        UNIT_LOCAL_DEMO_FLAG    (default "fallthrough-test-1.enabled")

import ConfidenceLocalProvider
import OpenFeature
import SwiftUI

@main
struct DemoApp: App {
    private static let env = ProcessInfo.processInfo.environment
    private let secret = env["CLIENT_SECRET"] ?? "mkjJruAATQWjeY7foFIWfVAcBWnci2YF"
    private let serverURL = URL(string: env["UNIT_LOCAL_SERVER"] ?? "http://127.0.0.1:8787")!
    private let demoFlag = env["UNIT_LOCAL_DEMO_FLAG"] ?? "fallthrough-test-1.enabled"

    var body: some Scene {
        WindowGroup {
            DemoView(clientSecret: secret, serverURL: serverURL, flagKey: demoFlag)
        }
    }
}

struct DemoView: View {
    let clientSecret: String
    let serverURL: URL
    let flagKey: String

    @State private var inputUnit: String = "user_42"
    @State private var status: Status = .idle
    @State private var resolvedValue: Bool = false
    @State private var resolvedReason: String = ""
    @State private var resolvedVariant: String = ""
    @State private var errorDetail: String = ""

    enum Status: Equatable {
        case idle
        case bootstrapping
        case resolving
        case ready
        case error
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 16) {
                statusHeader

                HStack {
                    Text("unit").font(.callout).foregroundStyle(.secondary)
                    TextField("user_42", text: $inputUnit)
                        .textFieldStyle(.roundedBorder)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    Button("Resolve") {
                        Task { await resolve(unit: inputUnit) }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(inputUnit.isEmpty || status == .resolving || status == .bootstrapping)
                }
                .padding(.horizontal)

                Spacer()

                resultBox

                Spacer()

                Text(serverURL.absoluteString)
                    .font(.footnote).foregroundStyle(.secondary)
            }
            .padding()
            .navigationTitle("Unit-local Demo")
            .task { await bootstrap() }
        }
    }

    @ViewBuilder
    private var statusHeader: some View {
        switch status {
        case .idle:           Text("Initialising…").font(.headline).foregroundStyle(.secondary)
        case .bootstrapping:  ProgressView("Bootstrapping local provider…")
        case .resolving:      ProgressView("Resolving flag…")
        case .ready:          Text("Ready").font(.headline).foregroundStyle(.green)
        case .error:          Text("Error").font(.headline).foregroundStyle(.red)
        }
    }

    @ViewBuilder
    private var resultBox: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("flag: \(flagKey)").font(.body.monospaced())
            Text("reason: \(resolvedReason)").font(.body.monospaced())
            Text("variant: \(resolvedVariant)").font(.body.monospaced())
            Text("value: \(resolvedValue ? "true" : "false")")
                .font(.body.monospaced())
                .foregroundStyle(resolvedValue ? .green : .red)
            if !errorDetail.isEmpty {
                Text("error: \(errorDetail)").font(.footnote.monospaced()).foregroundStyle(.red)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
        .background(Color.gray.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    private func bootstrap() async {
        guard status == .idle else { return }
        status = .bootstrapping
        do {
            let provider = try ConfidenceLocalProvider(clientSecret: clientSecret, serverURL: serverURL)
            await OpenFeatureAPI.shared.setProviderAndWait(provider: provider, initialContext: makeContext(unit: inputUnit))
            evaluate()
        } catch {
            status = .error
            errorDetail = String(describing: error)
        }
    }

    private func resolve(unit: String) async {
        status = .resolving
        await OpenFeatureAPI.shared.setEvaluationContext(evaluationContext: makeContext(unit: unit))
        evaluate()
    }

    private func makeContext(unit: String) -> EvaluationContext {
        ImmutableContext(
            targetingKey: unit,
            structure: ImmutableStructure(attributes: [
                "visitor_id": .string(unit),
                "country": .string("SE"),
            ])
        )
    }

    private func evaluate() {
        let client = OpenFeatureAPI.shared.getClient()
        let details = client.getDetails(key: flagKey, defaultValue: false)
        resolvedValue = details.value
        resolvedReason = details.reason ?? "nil"
        resolvedVariant = details.variant ?? ""
        if let code = details.errorCode {
            status = .error
            errorDetail = "errorCode=\(code) msg=\(details.errorMessage ?? "nil")"
        } else {
            status = .ready
            errorDetail = ""
        }
    }
}
