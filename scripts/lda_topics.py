#!/usr/bin/env python3
"""LDA topic modeling over rete-discovered communities.

Consumes the JSON emitted by `rete communities <file> --json`: a list of
communities, each with a `text` list of literal lexical values. Each community
is treated as ONE document (its text joined). We build a document-term matrix
(English stopwords) and fit Latent Dirichlet Allocation, then print, per topic,
its top words; and per community, its dominant topic + a short label.

rete supplies the two hard parts — the structural partition (Louvain
communities) and the per-community text extraction. LDA is a standard downstream
step; rete is not an ML engine.

Usage:
    rete communities papers.rete --json | python3 scripts/lda_topics.py --topics 3
    python3 scripts/lda_topics.py communities.json --topics 3 --top-words 8

Requires: scikit-learn  (pip install scikit-learn)
"""
import argparse
import json
import sys


def load_communities(path):
    """Read the communities JSON from a file path, or stdin if path is '-'."""
    if path == "-":
        data = json.load(sys.stdin)
    else:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    if not isinstance(data, list):
        sys.exit("error: expected a JSON array of communities")
    return data


def main():
    ap = argparse.ArgumentParser(
        description="LDA topic modeling over rete communities (reads `rete "
        "communities --json`)."
    )
    ap.add_argument(
        "input",
        nargs="?",
        default="-",
        help="communities JSON file, or '-' for stdin (default).",
    )
    ap.add_argument("--topics", type=int, default=3, help="number of LDA topics (default 3).")
    ap.add_argument(
        "--top-words", type=int, default=8, help="top words to show per topic (default 8)."
    )
    args = ap.parse_args()

    # scikit-learn is imported lazily so --help works without it installed.
    try:
        from sklearn.decomposition import LatentDirichletAllocation
        from sklearn.feature_extraction.text import CountVectorizer
    except ImportError:
        sys.exit("error: scikit-learn is required — install with `pip install scikit-learn`")

    communities = load_communities(args.input)

    # One document per community: join its literal text. Keep only non-empty.
    docs, labels = [], []
    for c in communities:
        text = " ".join(c.get("text", [])).strip()
        if text:
            docs.append(text)
            labels.append(c.get("community", len(labels)))

    if not docs:
        print("No text found in any community — nothing to model.")
        print("(Communities had no literal objects; topic modeling needs text.)")
        return

    if len(docs) < 2:
        print("Only one community with text — LDA needs at least two documents.")
        print(f"Community {labels[0]} text (first 200 chars): {docs[0][:200]}")
        return

    # min_df=1 is appropriate for tiny corpora; drop English stopwords.
    vectorizer = CountVectorizer(stop_words="english", min_df=1)
    dtm = vectorizer.fit_transform(docs)
    vocab = vectorizer.get_feature_names_out()
    n_features = len(vocab)
    if n_features == 0:
        print("No usable words after removing stopwords — nothing to model.")
        return

    # Clamp topics to what the data can support.
    n_topics = max(1, min(args.topics, len(docs), n_features))
    if n_topics != args.topics:
        print(f"(clamped topics from {args.topics} to {n_topics} for this corpus)\n")

    lda = LatentDirichletAllocation(
        n_components=n_topics, random_state=0, learning_method="batch", max_iter=50
    )
    doc_topics = lda.fit_transform(dtm)

    # Per-topic top words.
    top_n = min(args.top_words, n_features)
    topic_words = []
    print(f"== {n_topics} topic(s) over {len(docs)} community document(s) ==")
    for t, comp in enumerate(lda.components_):
        top_idx = comp.argsort()[: -top_n - 1 : -1]
        words = [vocab[i] for i in top_idx]
        topic_words.append(words)
        print(f"  topic {t}: {', '.join(words)}")

    # Per-community dominant topic + a short label (its topic's top 3 words).
    print("\n== community → dominant topic ==")
    for label, dist in zip(labels, doc_topics):
        dom = int(dist.argmax())
        short = ", ".join(topic_words[dom][:3])
        print(f"  community {label}: topic {dom}  [{short}]  (p={dist[dom]:.2f})")


if __name__ == "__main__":
    main()
