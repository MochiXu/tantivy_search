use tantivy::tokenizer::Token;

#[derive(Debug)]
pub struct IcuTokenStream<'a> {
    src: &'a str,
    result: Vec<&'a str>,
    // Begin with 1
    index: usize,
    token: Token,
}

impl<'a> IcuTokenStream<'a> {
    pub fn new(src: &'a str, result: Vec<&'a str>) -> Self {
        IcuTokenStream {
            src,
            result,
            index: 0,
            token: Token::default(),
        }
    }
}

impl<'a> ::tantivy::tokenizer::TokenStream for IcuTokenStream<'a> {
    fn advance(&mut self) -> bool {
        if self.index < self.result.len() {
            let current_word = self.result[self.index];
            let offset_from = current_word.as_ptr() as usize - self.src.as_ptr() as usize;
            let offset_to = offset_from + current_word.len();

            self.token = Token {
                offset_from,
                offset_to,
                position: self.index,
                text: current_word.to_string(),
                position_length: 1,
            };

            self.index += 1;
            true
        } else {
            false
        }
    }

    fn token(&self) -> &Token {
        &self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.token
    }
}
