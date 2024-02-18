use std::io::{BufRead, Seek};

use ratchet::prelude::*;
use ratchet_loader::GGMLModel;
use ratchet_nn::{Embedding, LayerNorm, Module};

use crate::{ResidualAttentionBlock, ResidualAttentionBlockInputs, Whisper};

#[derive(Debug)]
pub(crate) struct DecoderStem {
    pub token_embed: Embedding,
    pub pos_embed: Tensor,
}

impl DecoderStem {
    pub fn load<R: BufRead + Seek>(
        disk_model: &GGMLModel<Whisper>,
        reader: &mut R,
        device: &Device,
    ) -> anyhow::Result<Self> {
        let mut lt = |name: &str| {
            let key = format!("decoder.{}", name);
            disk_model.load_tensor(&key, reader, device)
        };

        Ok(Self {
            token_embed: Embedding::new(lt("token_embedding.weight")?),
            pos_embed: lt("positional_embedding")?,
        })
    }
}

impl Module for DecoderStem {
    type Input = Tensor;

    fn forward(&self, input: &Self::Input) -> anyhow::Result<Tensor> {
        let num_tokens = input.shape()[input.rank() - 1];
        let sliced = self.pos_embed.slice(&[0..num_tokens, 0..384])?;
        self.token_embed.forward(input)?.add(&sliced)
    }
}

#[derive(Debug)]
pub struct WhisperDecoder {
    stem: DecoderStem,
    blocks: Vec<ResidualAttentionBlock>,
    mask: Tensor,
    ln_post: LayerNorm,
}

impl Module for WhisperDecoder {
    type Input = [Tensor; 2];

    fn forward(&self, input: &Self::Input) -> anyhow::Result<Tensor> {
        let [audio_ctx, tokens] = input;
        let mut x = self.stem.forward(tokens)?;
        for block in &self.blocks {
            let block_input = ResidualAttentionBlockInputs {
                x,
                xa: Some(audio_ctx.clone()),
                mask: Some(self.mask.clone()),
            };
            x = block.forward(&block_input)?;
        }
        x = self.ln_post.forward(&x)?;
        let logits = x.matmul(&self.stem.token_embed.weight.permute(&[1, 0])?)?;
        Ok(logits)
    }
}

impl WhisperDecoder {
    fn load_mask(n_ctx: usize, device: &Device) -> Tensor {
        let mask: Vec<_> = (0..n_ctx)
            .flat_map(|i| (0..n_ctx).map(move |j| if j > i { f32::NEG_INFINITY } else { 0f32 }))
            .collect();
        Tensor::from_data(mask, shape![n_ctx, n_ctx], device.clone())
    }

    pub fn load<R: BufRead + Seek>(
        disk_model: &GGMLModel<Whisper>,
        reader: &mut R,
        device: &Device,
    ) -> anyhow::Result<Self> {
        let hparams = &disk_model.header.hparams;
        let stem = DecoderStem::load(disk_model, reader, device)?;
        let (n_layers, n_heads) = (hparams.n_text_layer, hparams.n_text_head);

        let blocks = (0..n_layers)
            .fold(Vec::with_capacity(n_layers as _), |mut blocks, i| {
                blocks.push(ResidualAttentionBlock::load(
                    disk_model,
                    reader,
                    i as _,
                    n_heads as _,
                    "decoder",
                    true,
                    device,
                ));
                blocks
            })
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;

        let mut lt = |name: &str| {
            let key = format!("decoder.ln.{}", name);
            disk_model.load_tensor(&key, reader, device)
        };

        Ok(Self {
            stem,
            blocks,
            mask: Self::load_mask(hparams.n_text_ctx as _, device),
            ln_post: LayerNorm::new(lt("weight")?, Some(lt("bias")?), 1e-5),
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::path::PathBuf;

    use crate::{SpectrogramGenerator, Whisper, WhisperDecoder, WhisperEncoder};
    use hf_hub::api::sync::Api;
    use ndarray::{s, Axis};
    use ndarray_stats::QuantileExt;
    use ratchet::{shape, Device, DeviceRequest, Tensor};
    use ratchet_loader::GGMLCompatible;
    use ratchet_nn::Module;
    use tokenizers::Tokenizer;

    pub fn load_npy(path: PathBuf) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        npyz::NpyFile::new(&bytes[..]).unwrap().into_vec().unwrap()
    }

    fn load_sample(audio: PathBuf) -> Vec<f32> {
        let mut reader = hound::WavReader::open(audio).unwrap();
        reader
            .samples::<i16>()
            .map(|x| x.unwrap() as f32 / 32768.0)
            .collect::<Vec<_>>()
    }

    #[test]
    fn decoder_matches() -> anyhow::Result<()> {
        let api = Api::new().unwrap();
        let model = api.model("ggerganov/whisper.cpp".to_string());
        let path = model.get("ggml-tiny.bin").unwrap();

        let tokenizer_repo = api.model("openai/whisper-tiny".to_string());
        let tokenizer_path = tokenizer_repo.get("tokenizer.json").unwrap();

        let dataset = api.dataset("FL33TW00D-HF/ratchet-util".to_string());
        let audio = dataset.get("jfk.wav").unwrap();
        let mels = dataset.get("mel_filters.npy").unwrap();

        let mut reader = std::io::BufReader::new(std::fs::File::open(path).unwrap());
        let gg_disk = Whisper::load_ggml(&mut reader).unwrap();
        assert_eq!(gg_disk.tensors.len(), 167);

        let device = Device::request_device(DeviceRequest::GPU).unwrap();
        let generator = SpectrogramGenerator::new(load_npy(mels));
        let log_mel = generator.generate(load_sample(audio))?.to(&device)?;

        let encoder = WhisperEncoder::load(&gg_disk, &mut reader, &device)?;
        let decoder = WhisperDecoder::load(&gg_disk, &mut reader, &device)?;

        let audio_ctx = encoder.forward(&log_mel.slice(&[0..1, 0..80, 0..3000])?)?;
        audio_ctx.resolve()?;

        let mut tokens = vec![50258, 50259, 50359];
        let mut token = -1;
        while token != 50257 {
            let token_t =
                Tensor::from_data(tokens.clone(), shape![1, tokens.len()], device.clone());
            let result = decoder.forward(&[audio_ctx.clone(), token_t])?;
            result.resolve()?;

            let our_logits = result.to(&Device::CPU)?;
            let nd_logits = our_logits.to_ndarray_view::<f32>();
            let sliced = nd_logits.slice(s![.., -1.., ..51865]).remove_axis(Axis(1));

            token = sliced
                .map_axis(Axis(1), |row| row.argmax_skipnan().unwrap())
                .iter()
                .map(|&x| x as i32)
                .collect::<Vec<_>>()[0];
            println!("Token: {}", token);
            tokens.push(token);
        }

        let tokenizer = Tokenizer::from_file(tokenizer_path).unwrap();
        let u32_tokens: Vec<_> = tokens.iter().map(|&x| x as u32).collect();
        let decoded = tokenizer.decode(&u32_tokens, true).unwrap();
        println!("{}", decoded);

        Ok(())
    }
}