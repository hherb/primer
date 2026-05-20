// Minimal proof: mic -> SpeechAnalyzer/SpeechTranscriber -> stdout partials + finals.
// Each result line carries timing:
//   [wall_ms] tag  lag=Xms  range=A..Bms  fin=Cms  "text"
// where wall_ms is wall-clock ms since first audio frame, range is the audio
// time the result covers (per SpeechTranscriber.Result.range), fin is
// resultsFinalizationTime, and lag = wall_ms - range_end_ms (positive means
// transcript trails the audio it describes).
//
// Usage:   swift run macos26_speech [bcp47-locale]
// Default: en-US. Try de-DE; on first run for a locale, the model downloads.
// Stop:    Ctrl+C.

import AVFoundation
import CoreMedia
import Foundation
import Speech

@main
struct Spike {
    static func main() async {
        do {
            try await run()
        } catch {
            FileHandle.standardError.write(Data("error: \(error)\n".utf8))
            exit(1)
        }
    }

    static func run() async throws {
        let localeId = CommandLine.arguments.dropFirst().first ?? "en-US"
        let locale = Locale(identifier: localeId)
        log("locale: \(localeId)")

        let transcriber = SpeechTranscriber(
            locale: locale,
            preset: .progressiveTranscription
        )
        try await ensureModel(for: transcriber, locale: locale)

        // SpeechDetector gates the transcriber so silence skips the model (power saving).
        // Its Result stream "currently only supports error handling" per Apple docs, so
        // it's not a source of speech-start/end events — derive those from the transcriber
        // (first volatile partial after silence = speech-start).
        let detector = SpeechDetector(
            detectionOptions: .init(sensitivityLevel: .medium),
            reportResults: false
        )

        let analyzer = SpeechAnalyzer(modules: [detector, transcriber])
        guard let analyzerFormat = await SpeechAnalyzer.bestAvailableAudioFormat(
            compatibleWith: [transcriber]
        ) else {
            throw SpikeError.noAudioFormat
        }
        log("analyzer format: \(analyzerFormat)")

        let (inputSequence, inputBuilder) = AsyncStream<AnalyzerInput>.makeStream()

        let audioEngine = AVAudioEngine()
        let micFormat = audioEngine.inputNode.outputFormat(forBus: 0)
        log("mic format: \(micFormat)")

        guard let converter = AVAudioConverter(from: micFormat, to: analyzerFormat) else {
            throw SpikeError.noConverter
        }

        let anchor = TimingAnchor()

        audioEngine.inputNode.installTap(
            onBus: 0,
            bufferSize: 4096,
            format: micFormat
        ) { buffer, _ in
            anchor.markFirstAudio()
            let ratio = analyzerFormat.sampleRate / micFormat.sampleRate
            let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 1024
            guard let out = AVAudioPCMBuffer(
                pcmFormat: analyzerFormat,
                frameCapacity: capacity
            ) else { return }

            var fed = false
            var convError: NSError?
            let status = converter.convert(to: out, error: &convError) { _, statusPtr in
                if fed {
                    statusPtr.pointee = .noDataNow
                    return nil
                }
                fed = true
                statusPtr.pointee = .haveData
                return buffer
            }
            if status == .error || convError != nil {
                logErr("convert: \(String(describing: convError))")
                return
            }
            inputBuilder.yield(AnalyzerInput(buffer: out))
        }

        try audioEngine.start()
        try await analyzer.start(inputSequence: inputSequence)

        log("listening — press Ctrl+C to stop")
        print("---")

        for try await result in transcriber.results {
            let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
            let wallMs = anchor.elapsedMs() ?? 0
            let rangeStartMs = result.range.start.seconds * 1000
            let rangeEndMs = result.range.end.seconds * 1000
            let finMs = result.resultsFinalizationTime.seconds * 1000
            let lagMs = wallMs - rangeEndMs
            let tag = result.isFinal ? "FINAL" : "part "
            let line = String(
                format: "[%7.0fms] %@  lag=%6.0fms  range=%7.0f..%-7.0fms  fin=%7.0fms  %@",
                wallMs, tag, lagMs, rangeStartMs, rangeEndMs, finMs, text
            )
            print(line)
            fflush(stdout)
        }
    }

    static func ensureModel(for transcriber: SpeechTranscriber, locale: Locale) async throws {
        let bcp47 = locale.identifier(.bcp47)
        let supported = await SpeechTranscriber.supportedLocales
        guard supported.contains(where: { $0.identifier(.bcp47) == bcp47 }) else {
            let avail = supported.map { $0.identifier(.bcp47) }.sorted().joined(separator: ", ")
            logErr("supported locales: \(avail)")
            throw SpikeError.localeNotSupported(bcp47)
        }
        let installed = await SpeechTranscriber.installedLocales
        if installed.contains(where: { $0.identifier(.bcp47) == bcp47 }) {
            log("model for \(bcp47) already installed")
            return
        }
        log("downloading model for \(bcp47) (one-time)...")
        guard let req = try await AssetInventory.assetInstallationRequest(
            supporting: [transcriber]
        ) else {
            throw SpikeError.noInstallationRequest
        }
        try await req.downloadAndInstall()
        log("download complete")
    }
}

/// One-time-set wall-clock anchor. Set from the audio tap thread on the
/// first buffer; read from the result-consumer task. NSLock-guarded so
/// the cross-thread access is sound under Swift 6 strict concurrency.
final class TimingAnchor: @unchecked Sendable {
    private let lock = NSLock()
    private var firstAudio: CFAbsoluteTime?

    func markFirstAudio() {
        lock.lock(); defer { lock.unlock() }
        if firstAudio == nil { firstAudio = CFAbsoluteTimeGetCurrent() }
    }

    func elapsedMs() -> Double? {
        lock.lock(); defer { lock.unlock() }
        guard let start = firstAudio else { return nil }
        return (CFAbsoluteTimeGetCurrent() - start) * 1000.0
    }
}

enum SpikeError: Error, CustomStringConvertible {
    case noAudioFormat
    case noConverter
    case localeNotSupported(String)
    case noInstallationRequest

    var description: String {
        switch self {
        case .noAudioFormat: return "no compatible analyzer audio format"
        case .noConverter: return "could not create AVAudioConverter"
        case .localeNotSupported(let id): return "locale '\(id)' not supported by SpeechTranscriber"
        case .noInstallationRequest: return "AssetInventory returned no installation request"
        }
    }
}

@inline(__always) private func log(_ msg: String) {
    FileHandle.standardError.write(Data("[spike] \(msg)\n".utf8))
}
@inline(__always) private func logErr(_ msg: String) {
    FileHandle.standardError.write(Data("[spike:err] \(msg)\n".utf8))
}
