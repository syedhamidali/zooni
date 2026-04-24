//! ML-based market regime classifier using an ONNX model (XGBoost + StandardScaler).
//!
//! The model was trained in Python (`ml/train_regime.py`) and exported to ONNX.
//! Features (in order, matching `ml/feature_names.txt`):
//!   adx, rsi, bb_width, atr_ratio, volume_ratio, returns_std_5d, returns_std_20d
//!
//! Returns `true` for **Ranging**, `false` for **Trending**.

use anyhow::{Context, Result};
use tract_onnx::prelude::*;

/// Number of input features expected by the ONNX model.
pub const N_FEATURES: usize = 7;

type OnnxPlan = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

/// A loaded ONNX regime classifier ready for single-bar inference.
pub struct MlRegimeClassifier {
    plan: OnnxPlan,
}

impl MlRegimeClassifier {
    /// Load the ONNX model from `path`.  Returns an error if the file does not
    /// exist or cannot be parsed — callers should fall back to ADX heuristics.
    pub fn load(path: &str) -> Result<Self> {
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .with_context(|| format!("loading ONNX model from '{path}'"))?
            .into_optimized()
            .context("optimising ONNX model")?
            .into_runnable()
            .context("building runnable ONNX plan")?;

        Ok(Self { plan: model })
    }

    /// Run inference for a single bar.
    ///
    /// # Arguments
    /// * `adx`            – ADX(14) value
    /// * `rsi`            – RSI(14) value
    /// * `bb_width`       – Bollinger bandwidth (20-bar)
    /// * `atr_ratio`      – ATR(14) / close
    /// * `volume_ratio`   – current volume / 20-bar average volume
    /// * `returns_std_5d` – rolling 5-bar log-return std-dev
    /// * `returns_std_20d`– rolling 20-bar log-return std-dev
    ///
    /// # Returns
    /// `true` if the model predicts **Ranging** (label 1), `false` for **Trending**.
    pub fn predict(
        &self,
        adx: f32,
        rsi: f32,
        bb_width: f32,
        atr_ratio: f32,
        volume_ratio: f32,
        returns_std_5d: f32,
        returns_std_20d: f32,
    ) -> Result<bool> {
        // Build a 1×7 f32 tensor
        let features: Vec<f32> = vec![
            adx,
            rsi,
            bb_width,
            atr_ratio,
            volume_ratio,
            returns_std_5d,
            returns_std_20d,
        ];

        let input: Tensor = tract_ndarray::Array2::from_shape_vec(
            (1, N_FEATURES),
            features,
        )
        .context("building input array")?
        .into();

        let outputs = self
            .plan
            .run(tvec![input.into()])
            .context("running ONNX inference")?;

        // The XGBoost ONNX model produces two outputs:
        //   output[0] – predicted class label (int64 scalar or 1-d tensor)
        //   output[1] – probability map (sequence of {class: prob} maps)
        //
        // We read the class label from output[0].
        let label_tensor = outputs[0]
            .to_array_view::<i64>()
            .context("reading label output as i64")?;

        // First element is the predicted class (0 = Trending, 1 = Ranging)
        let predicted_class = *label_tensor
            .iter()
            .next()
            .context("empty label output tensor")?;

        Ok(predicted_class == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// When the ONNX file is absent, `load` should return an error (not panic).
    #[test]
    fn load_nonexistent_file_returns_err() {
        let result = MlRegimeClassifier::load("/tmp/does_not_exist_regime.onnx");
        assert!(result.is_err(), "expected Err for missing ONNX file");
    }
}
