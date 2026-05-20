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

public enum Macos26PipelineError: Error {
    case localeNotSupported(String)
    case noAnalyzerFormat
    case noInstallationRequest
    case streamClosed
}

// NOTE: ResultEvent is declared in the swift-bridge generated file
// (generated/Macos26Pipeline/Macos26Pipeline.swift). Do NOT re-declare it here.
// The generated struct has snake_case fields and uses RustString for the text.

/// Owns the SpeechAnalyzer + SpeechTranscriber + SpeechDetector trio.
/// Audio is pushed by Rust via feedAudio. Results are pulled by Rust
/// via nextResultBridge, which awaits the next item on transcriber.results.
public final class Macos26Pipeline {
    private let analyzer: SpeechAnalyzer
    private let transcriber: SpeechTranscriber
    private let inputContinuation: AsyncStream<AnalyzerInput>.Continuation
    private let analyzerFormat: AVAudioFormat
    private var resultsIterator: AsyncThrowingStream<SpeechTranscriber.Result, Error>.AsyncIterator?

    // Tracks an in-flight call to the underlying iterator. nextResult() is
    // non-cancellation-safe at the Swift level (consuming an AsyncIterator
    // twice concurrently fatal-errors), but Rust's tokio::select! routinely
    // drops the future when other branches fire. Cache the Task so cancelled
    // callers can be replaced by the next iteration's caller awaiting the
    // same in-flight value — only one iter.next() runs at a time.
    //
    // Cached in-flight iterator advance. Cleanup happens INSIDE the
    // Task body so cancellation of the outer Rust future (via
    // tokio::select! dropping the awaiting future) does not race the
    // cleanup against the still-running iterator advance.
    private var nextResultTask: Task<ResultEvent?, Error>?

    // Private initializer — external callers use the static factory create(localeBcp47:).
    private init(
        analyzer: SpeechAnalyzer,
        transcriber: SpeechTranscriber,
        inputContinuation: AsyncStream<AnalyzerInput>.Continuation,
        analyzerFormat: AVAudioFormat,
        resultsIterator: AsyncThrowingStream<SpeechTranscriber.Result, Error>.AsyncIterator
    ) {
        self.analyzer = analyzer
        self.transcriber = transcriber
        self.inputContinuation = inputContinuation
        self.analyzerFormat = analyzerFormat
        self.resultsIterator = resultsIterator
    }

    /// Async factory exposed to Rust via associated_to = Macos26Pipeline.
    /// Maps to Swift: Macos26Pipeline.create(localeBcp47:).
    public static func create(localeBcp47: String) async throws -> Macos26Pipeline {
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

        let iter = AsyncThrowingStream<SpeechTranscriber.Result, Error> { cont in
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

        return Macos26Pipeline(
            analyzer: analyzer,
            transcriber: transcriber,
            inputContinuation: inputContinuation,
            analyzerFormat: fmt,
            resultsIterator: iter
        )
    }

    /// Sample rate the analyzer wants its input PCM at (typically 16 kHz).
    public func analyzerSampleRate() -> Double {
        return analyzerFormat.sampleRate
    }

    /// Push one PCM chunk into the analyzer. samples is mono Float32
    /// at the analyzer's preferred rate. Rust resamples upstream.
    /// Accepts RustVec<Float> as passed by swift-bridge from Vec<f32>.
    public func feedAudio(samples: RustVec<Float>) {
        let count = Int(samples.len())
        guard let buffer = AVAudioPCMBuffer(
            pcmFormat: analyzerFormat,
            frameCapacity: AVAudioFrameCount(count)
        ) else { return }
        buffer.frameLength = AVAudioFrameCount(count)
        if let channelData = buffer.floatChannelData {
            for i in 0..<count {
                channelData[0][i] = samples.get(index: UInt(i))!
            }
        }
        inputContinuation.yield(AnalyzerInput(buffer: buffer))
    }

    /// Pull the next transcriber result, awaiting if necessary. Returns
    /// nil once the underlying stream completes (analyzer stopped).
    ///
    /// Cancellation-safe via Task caching: if Rust's tokio::select! drops
    /// the awaiting future, the cached Task continues running against the
    /// iterator (so iter.next() is only ever called once at a time). The
    /// next call from Rust awaits the same Task and receives the value it
    /// was about to produce — no double-advance of the iterator.
    private func nextResult() async throws -> ResultEvent? {
        if let existing = nextResultTask {
            return try await existing.value
        }
        // The cleanup of `nextResultTask = nil` lives INSIDE the inner
        // Task body so it runs when the iterator advance actually
        // completes — NOT when the outer wrapper is cancelled by
        // tokio::select! dropping the future. This eliminates the race
        // where a cancelled outer wrapper's defer cleared the cached
        // Task while it was still running, allowing the next call to
        // create a second concurrent iter.next() and fatal-error in
        // AsyncStreamBuffer.swift:508.
        let task = Task<ResultEvent?, Error> { [weak self] in
            guard let self = self else { return nil }
            defer { self.nextResultTask = nil }
            guard var iter = self.resultsIterator else { return nil }
            defer { self.resultsIterator = iter }
            guard let result = try await iter.next() else { return nil }
            let text = String(result.text.characters)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let startMs = UInt64(max(0, result.range.start.seconds * 1000))
            let endMs = UInt64(max(0, result.range.end.seconds * 1000))
            return ResultEvent(
                text: RustString(text),
                is_final: result.isFinal,
                range_start_ms: startMs,
                range_end_ms: endMs,
                stream_done: false
            )
        }
        nextResultTask = task
        return try await task.value
    }

    /// Non-throwing bridge wrapper around nextResult().
    /// Returns a sentinel ResultEvent with stream_done=true on stream
    /// completion or any error. swift-bridge 0.1.x cannot bridge
    /// Option<SharedStruct> from async Swift methods, so we use a
    /// sentinel value instead of returning Optional.
    public func nextResultBridge() async -> ResultEvent {
        guard let event = try? await nextResult() else {
            return ResultEvent(text: RustString(""), is_final: false, range_start_ms: 0, range_end_ms: 0, stream_done: true)
        }
        return event
    }

    /// Stop the analyzer and tear down the pipeline.
    public func stop() async {
        inputContinuation.finish()
        try? await analyzer.finalizeAndFinishThroughEndOfInput()
    }
}

/// Async factory function bridged to Rust via swift-bridge.
/// Panics (fatalError) on any error because swift-bridge 0.1.59 cannot
/// generate valid code for Option<OpaqueType> or Result<OpaqueType, ...>
/// in extern "Swift" function positions. Callers should verify locale
/// support before invoking. Maps to bridge.rs: `async fn create`.
///
/// Argument label uses snake_case (locale_bcp47) to match swift-bridge output;
/// parameter type is RustString as passed by the generated bridge glue.
public func macos26PipelineCreate(locale_bcp47: RustString) async -> Macos26Pipeline {
    do {
        return try await Macos26Pipeline.create(localeBcp47: locale_bcp47.toString())
    } catch {
        fatalError("macos26PipelineCreate failed: \(error)")
    }
}
