"""Online decoder for synthetic PCM; no microphone, network, or application state."""
from importlib.metadata import version

import numpy as np
import sherpa_onnx

RATE = 16000


class Sherpa:
    engine_id = "sherpa-onnx-zipformer-en-20m-int8"
    version = version("sherpa-onnx")

    def __init__(self, model_dir, threads):
        self.files = {
            "tokens": model_dir / "tokens.txt",
            "encoder": model_dir / "encoder-epoch-99-avg-1.int8.onnx",
            "decoder": model_dir / "decoder-epoch-99-avg-1.onnx",
            "joiner": model_dir / "joiner-epoch-99-avg-1.int8.onnx",
        }
        if any(not path.is_file() for path in self.files.values()):
            raise FileNotFoundError("Missing model file for Sherpa 20M int8 recipe")
        self.recognizer = sherpa_onnx.OnlineRecognizer.from_transducer(
            **{key: str(path) for key, path in self.files.items()},
            num_threads=threads, provider="cpu", sample_rate=RATE, feature_dim=80,
            decoding_method="greedy_search",
        )

    def stream(self):
        return self.recognizer.create_stream()

    def advance(self, stream, pcm):
        samples = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768
        stream.accept_waveform(RATE, samples)
        self._drain(stream)

    def finish(self, stream):
        stream.input_finished()
        self._drain(stream)
        return self.recognizer.get_result(stream).strip()

    def _drain(self, stream):
        while self.recognizer.is_ready(stream):
            self.recognizer.decode_stream(stream)

    def recognize(self, pcm):
        stream = self.stream()
        self.advance(stream, pcm)
        return self.finish(stream)
