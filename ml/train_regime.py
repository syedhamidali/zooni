#!/usr/bin/env python3
"""
Train a XGBoost regime classifier from historical OHLCV data.

Features:  ADX(14), RSI(14), BB_width(20), ATR/close, volume_ratio, returns_std_5d, returns_std_20d
Label:     1 = Ranging  (next 5 bars stay within ±1.5×ATR of current close)
           0 = Trending

Exports the model to ml/regime_classifier.onnx using skl2onnx.
"""

import os
import sys
import warnings
warnings.filterwarnings("ignore")

import numpy as np
import pandas as pd
import yfinance as yf
from sklearn.preprocessing import StandardScaler
from sklearn.metrics import classification_report, accuracy_score
from xgboost import XGBClassifier
import onnxmltools
from onnxmltools.convert import convert_xgboost
from onnxmltools.convert.common.data_types import FloatTensorType as OnnxFloatTensorType
from skl2onnx import convert_sklearn
from skl2onnx.common.data_types import FloatTensorType

# ── Config ──────────────────────────────────────────────────────────────────
SYMBOLS   = ["BTC-USD", "AAPL"]
PERIOD    = "2y"
INTERVAL  = "1d"
ADX_PERIOD  = 14
RSI_PERIOD  = 14
BB_PERIOD   = 20
ATR_PERIOD  = 14
VOL_MA      = 20
STD_SHORT   = 5
STD_LONG    = 20
LABEL_BARS  = 5
ATR_MULT    = 1.5
TEST_SIZE   = 0.20
OUT_DIR     = os.path.dirname(os.path.abspath(__file__))  # ml/

# ── Indicator helpers ────────────────────────────────────────────────────────

def true_range(high: pd.Series, low: pd.Series, close: pd.Series) -> pd.Series:
    prev_close = close.shift(1)
    tr = pd.concat(
        [high - low, (high - prev_close).abs(), (low - prev_close).abs()], axis=1
    ).max(axis=1)
    return tr


def calc_atr(high, low, close, period=14) -> pd.Series:
    tr = true_range(high, low, close)
    return tr.ewm(span=period, min_periods=period).mean()


def calc_adx(high, low, close, period=14) -> pd.Series:
    tr   = true_range(high, low, close)
    up   = high.diff()
    down = -low.diff()
    plus_dm  = np.where((up > down) & (up > 0), up.values, 0.0)
    minus_dm = np.where((down > up) & (down > 0), down.values, 0.0)

    atr_s    = tr.ewm(span=period, min_periods=period).mean()
    plus_di  = 100 * pd.Series(plus_dm, index=high.index).ewm(span=period, min_periods=period).mean() / atr_s
    minus_di = 100 * pd.Series(minus_dm, index=high.index).ewm(span=period, min_periods=period).mean() / atr_s
    dx       = (100 * (plus_di - minus_di).abs() / (plus_di + minus_di + 1e-9))
    adx      = dx.ewm(span=period, min_periods=period).mean()
    return adx


def calc_rsi(close, period=14) -> pd.Series:
    delta = close.diff()
    gain  = delta.clip(lower=0).ewm(span=period, min_periods=period).mean()
    loss  = (-delta.clip(upper=0)).ewm(span=period, min_periods=period).mean()
    rs    = gain / (loss + 1e-9)
    return 100 - 100 / (1 + rs)


def calc_bb_width(close, period=20) -> pd.Series:
    ma    = close.rolling(period).mean()
    sigma = close.rolling(period).std()
    upper = ma + 2 * sigma
    lower = ma - 2 * sigma
    return (upper - lower) / (ma + 1e-9)


# ── Feature engineering ──────────────────────────────────────────────────────

def build_features(df: pd.DataFrame) -> pd.DataFrame:
    o, h, l, c, v = df["Open"], df["High"], df["Low"], df["Close"], df["Volume"]

    atr  = calc_atr(h, l, c, ATR_PERIOD)
    adx  = calc_adx(h, l, c, ADX_PERIOD)
    rsi  = calc_rsi(c, RSI_PERIOD)
    bbw  = calc_bb_width(c, BB_PERIOD)
    vol_ma     = v.rolling(VOL_MA).mean()
    vol_ratio  = v / (vol_ma + 1e-9)
    log_ret    = np.log(c / c.shift(1))
    ret_std_5  = log_ret.rolling(STD_SHORT).std()
    ret_std_20 = log_ret.rolling(STD_LONG).std()
    atr_ratio  = atr / (c + 1e-9)

    # Label: 1 = Ranging — next LABEL_BARS bars stay within ±ATR_MULT*ATR
    ranging = pd.Series(False, index=c.index)
    atr_arr = atr.values
    c_arr   = c.values
    n       = len(c_arr)
    for i in range(n - LABEL_BARS):
        band = ATR_MULT * atr_arr[i]
        future = c_arr[i + 1 : i + 1 + LABEL_BARS]
        ranging.iloc[i] = bool(np.all(np.abs(future - c_arr[i]) <= band))

    feat = pd.DataFrame({
        "adx":           adx,
        "rsi":           rsi,
        "bb_width":      bbw,
        "atr_ratio":     atr_ratio,
        "volume_ratio":  vol_ratio,
        "returns_std_5d":  ret_std_5,
        "returns_std_20d": ret_std_20,
        "label":         ranging.astype(int),
    })
    return feat


# ── Download & combine ───────────────────────────────────────────────────────

def download_data() -> pd.DataFrame:
    frames = []
    for sym in SYMBOLS:
        print(f"  Downloading {sym} ...")
        raw = yf.download(sym, period=PERIOD, interval=INTERVAL, progress=False, auto_adjust=True)
        if raw.empty:
            print(f"  WARNING: no data for {sym}, skipping")
            continue
        # yfinance may return MultiIndex columns when auto_adjust=True — flatten them
        if isinstance(raw.columns, pd.MultiIndex):
            raw.columns = [col[0] for col in raw.columns]
        raw = raw[["Open", "High", "Low", "Close", "Volume"]].dropna()
        feat = build_features(raw)
        frames.append(feat)
    if not frames:
        raise RuntimeError("No data downloaded — check your internet connection.")
    combined = pd.concat(frames, axis=0).dropna().reset_index(drop=True)
    return combined


# ── Train & export ───────────────────────────────────────────────────────────

FEATURE_NAMES = [
    "adx", "rsi", "bb_width", "atr_ratio",
    "volume_ratio", "returns_std_5d", "returns_std_20d",
]


def main():
    print("=== Regime Classifier Training ===")
    print(f"Symbols: {SYMBOLS}  |  Period: {PERIOD}  |  Interval: {INTERVAL}")

    data = download_data()
    print(f"Total samples after feature engineering: {len(data)}")
    print(f"Label distribution:\n{data['label'].value_counts()}\n")

    X = data[FEATURE_NAMES].values.astype(np.float32)
    y = data["label"].values.astype(int)

    # Train/test split (chronological — no shuffle to avoid look-ahead)
    split = int(len(X) * (1 - TEST_SIZE))
    X_train, X_test = X[:split], X[split:]
    y_train, y_test = y[:split], y[split:]

    print(f"Train: {len(X_train)}  |  Test: {len(X_test)}")

    # Scale features (fit on train, apply to test)
    scaler = StandardScaler()
    X_train_s = scaler.fit_transform(X_train).astype(np.float32)
    X_test_s  = scaler.transform(X_test).astype(np.float32)

    clf = XGBClassifier(
        n_estimators=200,
        max_depth=4,
        learning_rate=0.05,
        subsample=0.8,
        colsample_bytree=0.8,
        eval_metric="logloss",
        random_state=42,
    )
    clf.fit(X_train_s, y_train)
    y_pred = clf.predict(X_test_s)

    acc = accuracy_score(y_test, y_pred)
    print(f"Test accuracy: {acc:.4f}")
    print("\nClassification report:")
    print(classification_report(y_test, y_pred, target_names=["Trending", "Ranging"]))

    # ── Export to ONNX ────────────────────────────────────────────────────────
    # We wrap the scaler (StandardScaler → sklearn ONNX) and the XGBoost model
    # (XGBClassifier → onnxmltools) into a single pipeline ONNX graph.
    # The simplest portable approach: build a two-step ONNX by composing the
    # scaler ONNX and the XGB ONNX.
    n_features = len(FEATURE_NAMES)

    # 1. Convert StandardScaler with skl2onnx
    scaler_onnx = convert_sklearn(
        scaler,
        "scaler",
        [("input", FloatTensorType([None, n_features]))],
        target_opset=15,
    )

    # 2. Convert XGBClassifier with onnxmltools
    xgb_onnx = convert_xgboost(
        clf,
        "xgb",
        [("input", OnnxFloatTensorType([None, n_features]))],
        target_opset=15,
    )

    # 3. Compose using onnx graph utilities
    import onnx
    from onnx import helper

    # Re-wire: scaler output → xgb input
    # Scaler output name is "variable", xgb input name is "input"
    # We rename scaler output to match xgb's expected input name.
    scaler_model = onnx.load_from_string(scaler_onnx.SerializeToString())
    xgb_model    = onnx.load_from_string(xgb_onnx.SerializeToString())

    # Find scaler output name
    scaler_output_name = scaler_model.graph.output[0].name
    xgb_input_name     = xgb_model.graph.input[0].name

    # Build a combined graph: scaler nodes + xgb nodes, scaler output feeds xgb
    nodes = []
    # Scaler nodes
    for node in scaler_model.graph.node:
        new_outputs = [
            xgb_input_name if o == scaler_output_name else o
            for o in node.output
        ]
        new_node = helper.make_node(
            node.op_type,
            inputs=list(node.input),
            outputs=new_outputs,
            name=node.name,
            domain=node.domain,
        )
        for attr in node.attribute:
            new_node.attribute.append(attr)
        nodes.append(new_node)
    # XGB nodes
    for node in xgb_model.graph.node:
        nodes.append(node)

    # Combine initializers
    initializers = list(scaler_model.graph.initializer) + list(xgb_model.graph.initializer)

    # Combined graph: input = scaler input, outputs = xgb outputs
    combined_graph = helper.make_graph(
        nodes,
        "regime_classifier",
        inputs=list(scaler_model.graph.input),
        outputs=list(xgb_model.graph.output),
        initializer=initializers,
    )

    # Opsets
    opsets = {}
    for opset in list(scaler_model.opset_import) + list(xgb_model.opset_import):
        domain = opset.domain
        version = opset.version
        if domain not in opsets or opsets[domain] < version:
            opsets[domain] = version
    opset_imports = [
        helper.make_opsetid(domain, version) for domain, version in opsets.items()
    ]

    combined_model = helper.make_model(combined_graph, opset_imports=opset_imports)
    combined_model.ir_version = max(scaler_model.ir_version, xgb_model.ir_version)
    combined_model.doc_string = "XGBoost regime classifier (StandardScaler + XGBClassifier)"

    onnx_path = os.path.join(OUT_DIR, "regime_classifier.onnx")
    with open(onnx_path, "wb") as f:
        f.write(combined_model.SerializeToString())
    print(f"\nONNX model saved → {onnx_path}")
    print(f"  ONNX model size: {os.path.getsize(onnx_path) / 1024:.1f} KB")

    # ── Save feature names ────────────────────────────────────────────────────
    names_path = os.path.join(OUT_DIR, "feature_names.txt")
    with open(names_path, "w") as f:
        f.write("\n".join(FEATURE_NAMES) + "\n")
    print(f"Feature names saved → {names_path}")
    print("\nDone.")


if __name__ == "__main__":
    main()
