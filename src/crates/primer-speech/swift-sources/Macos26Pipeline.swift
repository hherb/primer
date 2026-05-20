// Swift sidecar for the macos-native-26 feature. Compiled into a static
// library by primer-speech's build.rs and linked statically. Reachable
// from Rust via the swift-bridge module at src/macos26/bridge.rs.
//
// Reference implementation: spikes/macos26_speech/Sources/macos26_speech/main.swift
// — same SpeechAnalyzer setup (.progressiveTranscription preset,
// .medium SpeechDetector sensitivity).

import AVFoundation
import CoreMedia
import Foundation
import Speech

/// Plain value type pushed to Rust per transcriber result. Strings cross
/// the bridge cleanly; AttributedString is reduced to plain text on the
/// Swift side so the Rust side never has to know about it.
public struct ResultEvent {
    public let text: String
    public let isFinal: Bool
    public let rangeStartMs: UInt64
    public let rangeEndMs: UInt64
}

public enum Macos26PipelineError: Error {
    case localeNotSupported(String)
    case noAnalyzerFormat
    case noInstallationRequest
    case streamClosed
}

/// Owns the SpeechAnalyzer + SpeechTranscriber + SpeechDetector trio.
/// Audio is pushed by Rust via `feedAudio`. Results are pulled by Rust
/// via `nextResult`, which awaits the next item on `transcriber.results`.
public final class Macos26Pipeline {
    private let analyzer: SpeechAnalyzer
    private let transcriber: SpeechTranscriber
    private let inputContinuation: AsyncStream<AnalyzerInput>.Continuation
    private let analyzerFormat: AVAudioFormat
    private var resultsIterator: AsyncThrowingStream<SpeechTranscriber.Result, Error>.AsyncIterator?

    public init(localeBcp47: String) async throws {
        let locale = Locale(identifier: localeBcp47)

        let supported = await SpeechTranscriber.supportedLocales
        guard supported.contains(where: { $0.identifier(.bcp47) == localeBcp47 }) else {
            throw Macos26PipelineError.localeNotSupported(localeBcp47)
        }

        let installed = await SpeechTranscriber.installedLocales
        let transcriber = SpeechTranscriber(
            locale: locale,
            preset: .progressiveTranscription
        )
        if !installed.contains(where: { $0.identifier(.bcp47) == localeBcp47 }) {
            // Apple's OS-managed download. No UI; the friction-free
            // demo policy is intentional (see spec, "Asset download").
            guard let req = try await AssetInventory.assetInstallationRequest(
                supporting: [transcriber]
            ) else {
                throw Macos26PipelineError.noInstallationRequest
            }
            try await req.downloadAndInstall()
        }

        let detector = SpeechDetector(
            detectionOptions: .init(sensitivityLevel: .medium),
            reportResults: false
        )
        let analyzer = SpeechAnalyzer(modules: [detector, transcriber])
        guard let fmt = await SpeechAnalyzer.bestAvailableAudioFormat(
            compatibleWith: [transcriber]
        ) else {
            throw Macos26PipelineError.noAnalyzerFormat
        }

        let (inputStream, inputContinuation) = AsyncStream<AnalyzerInput>.makeStream()
        try await analyzer.start(inputSequence: inputStream)

        self.analyzer = analyzer
        self.transcriber = transcriber
        self.inputContinuation = inputContinuation
        self.analyzerFormat = fmt
        // Wrap transcriber.results in an AsyncThrowingStream so we have
        // a concrete iterator type we can drive from nextResult().
        self.resultsIterator = AsyncThrowingStream<SpeechTranscriber.Result, Error> { cont in
            let task = Task {
                do {
                    for try await r in transcriber.results {
                        cont.yield(r)
                    }
                    cont.finish()
                } catch {
                    cont.finish(throwing: error)
                }
            }
            cont.onTermination = { _ in task.cancel() }
        }.makeAsyncIterator()
    }

    /// Sample rate the analyzer wants its input PCM at (typically 16 kHz).
    public func analyzerSampleRate() -> Double {
        return analyzerFormat.sampleRate
    }

    /// Push one PCM chunk into the analyzer. `samples` is mono Float32
    /// at the analyzer's preferred rate. Rust resamples upstream.
    public func feedAudio(samples: [Float]) {
        guard let buffer = AVAudioPCMBuffer(
            pcmFormat: analyzerFormat,
            frameCapacity: AVAudioFrameCount(samples.count)
        ) else { return }
        buffer.frameLength = AVAudioFrameCount(samples.count)
        if let channelData = buffer.floatChannelData {
            samples.withUnsafeBufferPointer { src in
                channelData[0].update(from: src.baseAddress!, count: samples.count)
            }
        }
        inputContinuation.yield(AnalyzerInput(buffer: buffer))
    }

    /// Pull the next transcriber result, awaiting if necessary. Returns
    /// nil once the underlying stream completes (analyzer stopped).
    public func nextResult() async throws -> ResultEvent? {
        guard var iter = resultsIterator else { return nil }
        defer { resultsIterator = iter }
        guard let result = try await iter.next() else { return nil }
        let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
        let startMs = UInt64(max(0, result.range.start.seconds * 1000))
        let endMs = UInt64(max(0, result.range.end.seconds * 1000))
        return ResultEvent(
            text: text,
            isFinal: result.isFinal,
            rangeStartMs: startMs,
            rangeEndMs: endMs
        )
    }

    /// Stop the analyzer and tear down the pipeline.
    public func stop() async {
        inputContinuation.finish()
        try? await analyzer.finalizeAndFinishThroughEndOfInput()
    }
}
