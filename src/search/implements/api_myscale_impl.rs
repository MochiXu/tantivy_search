use std::collections::HashSet;
use tantivy::query::Bm25StatisticsProvider;
use tantivy::reader::multi_parts_statistics::MultiPartsStatistics;
use tantivy::schema::Field;
use tantivy::schema::FieldType;
use tantivy::schema::Schema;
use tantivy::schema::TextFieldIndexing;
use tantivy::tokenizer::BoxTokenStream;
use tantivy::tokenizer::TextAnalyzer;
use tantivy::Term;

use super::strategy::query_strategy::QueryExecutor;
use super::strategy::query_strategy::QueryStrategy;
use crate::common::errors::TantivySearchError;
use crate::ffi::DocWithFreq;
use crate::ffi::Statistics;
use crate::logger::logger_bridge::TantivySearchLogger;
use crate::search::bridge::index_reader_bridge::IndexReaderBridge;
use crate::search::implements::strategy::bm25_natural_language_query::BM25NaturalLanguageStrategy;
use crate::search::implements::strategy::bm25_standard_query::BM25StandardQueryStrategy;
use crate::RowIdWithScore;
use crate::DEBUG;
use crate::FFI_INDEX_SEARCHER_CACHE;
use crate::{common::constants::LOG_CALLBACK, ERROR};
use std::collections::HashMap;
use std::sync::Arc;

fn bm25_inner_search(
    index_path: &str,
    statistics: &Statistics,
    strategy: &dyn QueryStrategy<Vec<RowIdWithScore>>,
) -> Result<Vec<RowIdWithScore>, TantivySearchError> {
    // Get index_reader_bridge from CACHE
    let index_reader_bridge: Arc<IndexReaderBridge> = FFI_INDEX_SEARCHER_CACHE
        .get_index_reader_bridge(index_path.to_string())
        .map_err(|e| {
            ERROR!(function:"bm25_inner_search", "{}", e);
            TantivySearchError::InternalError(e)
        })?;

    // Choose query strategy to construct query executor.
    let query_executor: QueryExecutor<'_, Vec<RowIdWithScore>> = QueryExecutor::new(strategy);

    let searcher = &mut index_reader_bridge.reader.searcher();

    // Not use statistics info.
    if statistics.docs_freq.len() == 0 {
        let result: Vec<RowIdWithScore> = query_executor.execute(searcher).map_err(|e| {
            ERROR!(function:"bm25_inner_search", "{}", e);
            TantivySearchError::IndexSearcherError(e)
        })?;

        return Ok(result);
    }

    let mut doc_freq_map: HashMap<Term, u64> = HashMap::new();
    for item in &statistics.docs_freq {
        let col_field = Field::from_field_id(item.field_id);
        let term: Term = Term::from_field_text(col_field, &item.term_str);
        doc_freq_map
            .entry(term)
            .and_modify(|count| *count += item.doc_freq)
            .or_insert(item.doc_freq);
    }

    // TODO Currently MyScale Can only execute bm25 search in one column.
    let mut total_num_tokens_map: HashMap<Field, u64> = HashMap::new();
    let field = Field::from_field_id(statistics.docs_freq[0].field_id);
    total_num_tokens_map
        .entry(field)
        .or_insert(statistics.total_num_tokens);

    let multi_parts_statistics = MultiPartsStatistics {
        doc_freq_map,
        total_num_tokens: total_num_tokens_map,
        total_num_docs: statistics.total_num_docs,
    };

    DEBUG!(function:"bm25_inner_search", "index_path:[{:?}], use MultiPartsStatistics[{:?}]", index_path, multi_parts_statistics);

    let _ = searcher.update_multi_parts_statistics(multi_parts_statistics);

    let result: Vec<RowIdWithScore> = query_executor.execute(searcher).map_err(|e| {
        ERROR!(function:"bm25_inner_search", "{}", e);
        TantivySearchError::IndexSearcherError(e)
    })?;

    Ok(result)
}

pub fn bm25_natural_language_search(
    index_path: &str,
    sentence: &str,
    topk: u32,
    u8_aived_bitmap: &Vec<u8>,
    query_with_filter: bool,
    operation_or: bool,
    statistics: &Statistics,
    need_doc: bool,
) -> Result<Vec<RowIdWithScore>, TantivySearchError> {
    // Choose query strategy to construct query executor.
    let bm25_natural_language_query: BM25NaturalLanguageStrategy<'_> =
        BM25NaturalLanguageStrategy {
            sentence,
            topk: &topk,
            u8_aived_bitmap,
            query_with_filter: &query_with_filter,
            need_doc: &need_doc,
            operation_or: &operation_or,
        };

    bm25_inner_search(index_path, statistics, &bm25_natural_language_query)
}

pub fn bm25_standard_search(
    index_path: &str,
    sentence: &str,
    topk: u32,
    u8_aived_bitmap: &Vec<u8>,
    query_with_filter: bool,
    operation_or: bool,
    statistics: &Statistics,
    need_doc: bool,
) -> Result<Vec<RowIdWithScore>, TantivySearchError> {
    // Choose query strategy to construct query executor.
    let bm25_standard_query: BM25StandardQueryStrategy<'_> = BM25StandardQueryStrategy {
        sentence,
        topk: &topk,
        query_with_filter: &query_with_filter,
        u8_aived_bitmap,
        need_doc: &need_doc,
        operation_or: &operation_or,
    };

    bm25_inner_search(index_path, statistics, &bm25_standard_query)
}

pub fn get_doc_freq(
    index_path: &str,
    sentence: &str,
) -> Result<Vec<DocWithFreq>, TantivySearchError> {
    // Get index_reader_bridge from CACHE
    let index_reader_bridge: Arc<IndexReaderBridge> = FFI_INDEX_SEARCHER_CACHE
        .get_index_reader_bridge(index_path.to_string())
        .map_err(|e| {
            ERROR!(function:"get_total_num_docs", "{}", e);
            TantivySearchError::InternalError(e)
        })?;
    let searcher = index_reader_bridge.reader.searcher();

    let schema: Schema = index_reader_bridge.index.schema();
    let mut terms: HashSet<Term> = HashSet::new();

    for (col_field, col_field_entry) in schema.fields() {
        let field_type = col_field_entry.field_type();
        if !field_type.is_indexed() {
            continue;
        }
        if let FieldType::Str(ref str_options) = field_type {
            let indexing_options: &TextFieldIndexing =
                str_options.get_indexing_options().ok_or_else(|| {
                    let error_msg: String = format!(
                        "column field:{} not indexed, but this error msg shouldn't display",
                        col_field_entry.name()
                    );
                    ERROR!(function:"get_doc_freq", "{}", error_msg);
                    TantivySearchError::InternalError(error_msg)
                })?;
            let mut text_analyzer: TextAnalyzer = searcher
                .index()
                .tokenizers()
                .get(indexing_options.tokenizer())
                .unwrap();
            let mut token_stream: BoxTokenStream<'_> = text_analyzer.token_stream(sentence);
            token_stream.process(&mut |token| {
                let term: Term = Term::from_field_text(col_field, &token.text);
                terms.insert(term);
            });
            // MyScale Only Support single column.
            break;
        }
    }
    let mut doc_with_freq_vector: Vec<DocWithFreq> = vec![];
    for term in terms {
        let doc_freq = searcher.doc_freq(&term).map_err(|e| {
            ERROR!(function:"get_doc_freq", "{}", e);
            TantivySearchError::TantivyError(e)
        })?;
        let doc_with_freq = DocWithFreq::new(
            term.value().as_str().unwrap_or("").to_string(),
            term.field().field_id(),
            doc_freq,
        );
        doc_with_freq_vector.push(doc_with_freq);
    }
    DEBUG!(function:"get_doc_freq", "index_path:[{:?}], doc_with_freq_vector:[{:?}]", index_path, doc_with_freq_vector);
    Ok(doc_with_freq_vector)
}

pub fn get_total_num_docs(index_path: &str) -> Result<u64, TantivySearchError> {
    // Get index_reader_bridge from CACHE
    let index_reader_bridge: Arc<IndexReaderBridge> = FFI_INDEX_SEARCHER_CACHE
        .get_index_reader_bridge(index_path.to_string())
        .map_err(|e| {
            ERROR!(function:"get_total_num_docs", "{}", e);
            TantivySearchError::InternalError(e)
        })?;
    let searcher = index_reader_bridge.reader.searcher();
    let total_num_docs = searcher
        .total_num_docs()
        .map_err(|e| TantivySearchError::TantivyError(e))?;
    DEBUG!(function:"get_total_num_docs", "index_path:[{:?}], total_num_docs is:[{:?}]", index_path, total_num_docs);
    Ok(total_num_docs)
}

pub fn get_total_num_tokens(index_path: &str) -> Result<u64, TantivySearchError> {
    // Get index_reader_bridge from CACHE
    let index_reader_bridge: Arc<IndexReaderBridge> = FFI_INDEX_SEARCHER_CACHE
        .get_index_reader_bridge(index_path.to_string())
        .map_err(|e| {
            ERROR!(function:"get_total_num_tokens", "{}", e);
            TantivySearchError::InternalError(e)
        })?;

    let schema: Schema = index_reader_bridge.index.schema();
    let searcher = index_reader_bridge.reader.searcher();

    for (col_field, col_field_entry) in schema.fields() {
        let field_type = col_field_entry.field_type();
        if !field_type.is_indexed() {
            continue;
        }
        if let FieldType::Str(_) = field_type {
            let field_total_num_tokens = searcher
                .total_num_tokens(col_field)
                .map_err(|e| TantivySearchError::TantivyError(e))?;

            DEBUG!(function:"get_total_num_tokens", "total_num_tokens for field-id:{} is {}", col_field.field_id(), field_total_num_tokens);
            // MyScale Only Support single column.
            return Ok(field_total_num_tokens);
        }
    }
    Ok(0u64)
}
