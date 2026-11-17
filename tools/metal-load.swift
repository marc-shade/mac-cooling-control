// Synthetic, repeatable Metal matrix workload for controller validation.
// Build: xcrun swiftc -O tools/metal-load.swift -o /tmp/padctl-metal-load
import Foundation
import Metal
import MetalPerformanceShaders

func run() throws {
    let duration = Double(CommandLine.arguments.dropFirst().first ?? "60") ?? 0
    guard duration >= 1 && duration <= 900 else {
        throw NSError(domain: "metal-load", code: 1, userInfo: [NSLocalizedDescriptionKey: "duration must be 1...900 seconds"])
    }
    guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
        throw NSError(domain: "metal-load", code: 2, userInfo: [NSLocalizedDescriptionKey: "Metal device unavailable"])
    }
    let dimension = 2048
    let count = dimension * dimension
    let descriptor = MPSMatrixDescriptor(rows: dimension, columns: dimension,
        rowBytes: dimension * MemoryLayout<Float>.stride, dataType: .float32)
    guard let aBuffer = device.makeBuffer(length: count * 4, options: .storageModeShared),
          let bBuffer = device.makeBuffer(length: count * 4, options: .storageModeShared),
          let cBuffer = device.makeBuffer(length: count * 4, options: .storageModeShared) else {
        throw NSError(domain: "metal-load", code: 3, userInfo: [NSLocalizedDescriptionKey: "Metal buffer allocation failed"])
    }
    aBuffer.contents().bindMemory(to: Float.self, capacity: count).initialize(repeating: 1, count: count)
    bBuffer.contents().bindMemory(to: Float.self, capacity: count).initialize(repeating: 2, count: count)
    let a = MPSMatrix(buffer: aBuffer, descriptor: descriptor)
    let b = MPSMatrix(buffer: bBuffer, descriptor: descriptor)
    let c = MPSMatrix(buffer: cBuffer, descriptor: descriptor)
    let multiply = MPSMatrixMultiplication(device: device, resultRows: dimension,
        resultColumns: dimension, interiorColumns: dimension)
    func emit(_ value: [String: Any]) throws {
        let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([10]))
    }
    try emit(["event": "config", "duration_seconds": duration, "dimension": dimension,
              "multiplications_per_batch": 4, "device": device.name,
              "workload": "synthetic MPS float32 matrix multiplication"])
    let start = ProcessInfo.processInfo.systemUptime
    var epochStart = start
    var epochCount = 0
    var totalCount = 0
    var gpuSeconds = 0.0
    repeat {
        try autoreleasepool {
            guard let commands = queue.makeCommandBuffer() else {
                throw NSError(domain: "metal-load", code: 4, userInfo: [NSLocalizedDescriptionKey: "command allocation failed"])
            }
            for _ in 0..<4 { multiply.encode(commandBuffer: commands, leftMatrix: a, rightMatrix: b, resultMatrix: c) }
            commands.commit()
            commands.waitUntilCompleted()
            if let error = commands.error { throw error }
            guard commands.status == .completed else {
                throw NSError(domain: "metal-load", code: 5, userInfo: [NSLocalizedDescriptionKey: "GPU command did not complete"])
            }
            gpuSeconds += commands.gpuEndTime - commands.gpuStartTime
            epochCount += 4
            totalCount += 4
        }
        let now = ProcessInfo.processInfo.systemUptime
        if now - epochStart >= 1 {
            let output = cBuffer.contents().bindMemory(to: Float.self, capacity: count)
            for index in stride(from: 0, to: count, by: 4093) {
                guard output[index] == Float(dimension * 2) else {
                    throw NSError(domain: "metal-load", code: 6, userInfo: [NSLocalizedDescriptionKey: "matrix result mismatch at \(index)"])
                }
            }
            try emit(["event": "sample", "elapsed_seconds": now - start,
                "matrix_multiplications_per_second": Double(epochCount) / (now - epochStart),
                "gpu_seconds": gpuSeconds, "thermal_state": ProcessInfo.processInfo.thermalState.rawValue,
                "sampled_result_check": "passed"])
            epochStart = now
            epochCount = 0
            gpuSeconds = 0
        }
    } while ProcessInfo.processInfo.systemUptime - start < duration
    try emit(["event": "complete", "total_multiplications": totalCount,
        "elapsed_seconds": ProcessInfo.processInfo.systemUptime - start])
}
do { try run() } catch {
    FileHandle.standardError.write(Data("error: \(error)\n".utf8))
    exit(1)
}
