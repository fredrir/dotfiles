import Foundation
import Metal

let source = """
#include <metal_stdlib>
using namespace metal;

kernel void fma_loop(device float *data [[buffer(0)]],
                     constant uint &rounds [[buffer(1)]],
                     uint index [[thread_position_in_grid]]) {
    float a = data[index];
    float b = 1.0000001f;
    float c = 0.0000001f;
    for (uint step = 0; step < rounds; step++) {
        a = fma(a, b, c);
        a = fma(a, b, c);
        a = fma(a, b, c);
        a = fma(a, b, c);
    }
    data[index] = a;
}
"""

let elements = 1 << 22
let rounds: UInt32 = 512
let flopsPerRound = 8.0

guard let device = MTLCreateSystemDefaultDevice() else {
    FileHandle.standardError.write("no Metal device\n".data(using: .utf8)!)
    exit(1)
}

guard let queue = device.makeCommandQueue(),
      let library = try? device.makeLibrary(source: source, options: nil),
      let function = library.makeFunction(name: "fma_loop"),
      let pipeline = try? device.makeComputePipelineState(function: function) else {
    FileHandle.standardError.write("could not build the Metal pipeline\n".data(using: .utf8)!)
    exit(1)
}

let length = elements * MemoryLayout<Float>.stride
guard let buffer = device.makeBuffer(length: length, options: .storageModeShared) else {
    FileHandle.standardError.write("could not allocate the Metal buffer\n".data(using: .utf8)!)
    exit(1)
}

let values = buffer.contents().bindMemory(to: Float.self, capacity: elements)
for index in 0..<elements {
    values[index] = 1.0
}

var count = rounds
let width = min(pipeline.maxTotalThreadsPerThreadgroup, 256)
let grid = MTLSize(width: elements, height: 1, depth: 1)
let group = MTLSize(width: width, height: 1, depth: 1)

func dispatch() -> Double {
    let start = DispatchTime.now().uptimeNanoseconds
    guard let commands = queue.makeCommandBuffer(),
          let encoder = commands.makeComputeCommandEncoder() else {
        return 0
    }
    encoder.setComputePipelineState(pipeline)
    encoder.setBuffer(buffer, offset: 0, index: 0)
    encoder.setBytes(&count, length: MemoryLayout<UInt32>.size, index: 1)
    encoder.dispatchThreads(grid, threadsPerThreadgroup: group)
    encoder.endEncoding()
    commands.commit()
    commands.waitUntilCompleted()
    return Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000_000
}

_ = dispatch()

var best = Double.greatestFiniteMagnitude
for _ in 0..<5 {
    let elapsed = dispatch()
    if elapsed > 0 && elapsed < best {
        best = elapsed
    }
}

let operations = Double(elements) * Double(rounds) * flopsPerRound
print(String(format: "%.2f", operations / best / 1_000_000_000))
