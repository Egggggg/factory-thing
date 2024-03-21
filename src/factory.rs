use std::{cell::RefCell, collections::HashMap, fmt::Display, rc::Rc};

use crate::{lang::parser::{Expr, InfixOp, Literal}, rate::Rate, Product, Recipe, RecipePart, Stream};

#[derive(Clone, Debug)]
pub struct Factory {
    pub products: HashMap<String, Rc<RefCell<Product>>>,
    pub recipes: HashMap<String, Rc<RefCell<Recipe>>>,
    pub streams: HashMap<String, Rc<RefCell<Stream>>>,
    pub modules: HashMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Product(String, Rc<RefCell<Product>>),
    Recipe(String, Rc<RefCell<Recipe>>),
    Stream(String, Rc<RefCell<Stream>>),
    RecipePart(RecipePart),
    Call(Box<Value>, Vec<Value>),
    InfixOp(Box<Value>, InfixOp, Box<Value>),
    MultRecipe(Box<Value>, usize),
    Int(isize),
    Float(f64),
    String(String),
    Bool(bool)
}

#[derive(Clone, Copy, Debug)]
pub enum FactoryError {
    Glorp,
    Gleep,
    Exists,
}

impl Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let content = match self {
            Self::Product(name, _) => format!("Product {{ {name} }}"),
            Self::Recipe(name, _) => format!("Recipe {{ {name} }}"),
            Self::Stream(name, _) => format!("Stream {{ {name} }}"),
            Self::Call(lhs, rhs) => {
                let rhs = rhs.iter().fold(String::new(), |acc, e| format!("{acc}, {e}"));
                format!("Call {{ {lhs}({}) }}", rhs)
            },
            Self::InfixOp(lhs, op, rhs) => format!("InfixOp {{ {lhs} {op} {rhs} }}"),
            e => format!("{:?}", e),
        };

        write!(f, "{}", content)
    }
}

impl Factory {
    pub fn new() -> Self {
        let products = HashMap::new();
        let recipes = HashMap::new();
        let streams = HashMap::new();
        let mut modules = HashMap::new();

        modules.insert("__BASE".to_owned(), 0);

        Self {
            products,
            recipes,
            streams,
            modules,
        }
    }

    pub fn add_mod(&mut self, ast: Vec<Expr>) -> Result<(), FactoryError> {
        for expr in ast {
            self.process_expr(expr)?;
        }

        Ok(())
    }

    fn process_expr(&mut self, expr: Expr) -> Result<Option<Value>, FactoryError> {
        match expr {
            Expr::Product { name } => {
                self.register_product(&name, "__BASE")?;
                Ok(None)
            },
            Expr::Recipe { name, inputs, outputs, period } => {
                self.register_recipe(&name, inputs, outputs, *period, "__BASE")?;
                Ok(None)
            },
            Expr::Assign { name, rhs } => {
                self.register_stream(&name, *rhs, "__BASE")?;
                Ok(None)
            }
            Expr::Ident(ident) => {
                let id = id(&ident, "__BASE");

                if let Some(product) = self.products.get(&id) {
                    Ok(Some(Value::Product(id, product.clone())))
                } else if let Some(recipe) = self.recipes.get(&id) {
                    Ok(Some(Value::Recipe(id, recipe.clone())))
                } else if let Some(stream) = self.streams.get(&id) {
                    Ok(Some(Value::Stream(id, stream.clone())))
                } else {
                    panic!("Undefined identifier: {ident}");
                }
            },
            Expr::Call { lhs, args } => {
                let lhs = self.process_expr(*lhs)?.unwrap();
                let mut args_out = Vec::with_capacity(args.len());

                for expr in args {
                    if let Some(value) = self.process_expr(expr)? {
                        args_out.push(value);
                    } else {
                        return Err(FactoryError::Glorp)
                    }
                }

                Ok(Some(Value::Call(Box::new(lhs), args_out)))
            }
            Expr::InfixOp { lhs, op, rhs } => {
                // unwrapping is fine here cause only expressions that return Some from this method should be put in an InfixOp
                let lhs = self.process_expr(*lhs)?.unwrap();
                let rhs = self.process_expr(*rhs)?.unwrap();

                Ok(Some(self.process_op(lhs, op, rhs)))
            },
            Expr::Literal(literal) => {
                Ok(Some(match literal {
                    Literal::Int(e) => Value::Int(e),
                    Literal::Float(e)  => Value::Float(e),
                    Literal::String(e) => Value::String(e),
                    Literal::Bool(e) => Value::Bool(e),
                }))
            }
            _ => todo!("{:?}", expr),
        }
    }

    fn process_op(&self, lhs: Value, op: InfixOp, rhs: Value) -> Value {
        match (lhs.clone(), op, rhs.clone()) {
            (Value::Product(_, product), InfixOp::Mul, Value::Int(amount))
            | (Value::Int(amount), InfixOp::Mul, Value::Product(_, product)) => {
                Value::RecipePart(RecipePart { product, amount: amount as usize })
            },
            (Value::Call(..), InfixOp::Mul, Value::Int(mult))
            | (Value::Int(mult), InfixOp::Mul, Value::Call(..)) => {
                Value::MultRecipe(Box::new(lhs), mult as usize)
            },
            (Value::MultRecipe(recipe, mult), InfixOp::Mul, Value::Int(mult2))
            | (Value::Int(mult2), InfixOp::Mul, Value::MultRecipe(recipe, mult)) => {
                Value::MultRecipe(recipe, mult * mult2 as usize)
            }
            (Value::InfixOp(lhs, op, rhs2), _, _) => {
                let lhs = self.process_op(*lhs, op, *rhs2);
                self.process_op(lhs, op, rhs)
            },
            (_, _, Value::InfixOp(lhs2, op, rhs)) => {
                let rhs = self.process_op(*lhs2, op, *rhs);
                self.process_op(lhs, op, rhs)
            }
            (Value::Int(lhs), InfixOp::Mul, Value::Int(rhs)) => Value::Int(lhs * rhs),
            (lhs, op, rhs) => panic!("Invalid operation: `{lhs:?} {op:?} {rhs:?}`"),
        }
    }

    fn register_product(&mut self, name: &str, module: &str) -> Result<(), FactoryError> {
        if self.products.get(name).is_none() {
            let module_id = self.get_module(module);
            let product_id = self.products.get("__next").map(|i| i.borrow().id).unwrap_or(0);

            self.products.insert("__next".to_owned(), Rc::new(RefCell::new(Product { id: product_id + 1, module: 0 })));
            self.products.insert(id(name, module), Rc::new(RefCell::new(Product { id: product_id, module: module_id })));

            Ok(())
        } else {
            Err(FactoryError::Exists)
        }
    }

    fn register_recipe(&mut self, name: &str, inputs: Vec<Expr>, outputs: Vec<Expr>, period: Expr, module: &str) -> Result<(), FactoryError> {
        if self.recipes.get(name).is_none() {
            let id = id(name, module);
            let inputs = self.parts_from_exprs(inputs)?;
            let outputs = self.parts_from_exprs(outputs)?;
            let period = self.usize_from_expr(period)?;
            let rate = Rate { amount: 1, freq: 1.0 / period as f64 };
            let recipe = Recipe {
                rate,
                inputs,
                outputs,
            };

            self.recipes.insert(id, Rc::new(RefCell::new(recipe)));

            Ok(())
        } else {
            Err(FactoryError::Exists)
        }
    }

    fn register_stream(&mut self, name: &str, expr: Expr, module: &str) -> Result<(), FactoryError> {
        if self.streams.get(name).is_none() {
            let id = id(name, module);
            let stream = self.stream_from_expr(expr)?;

            self.streams.insert(id, stream);

            Ok(())
        } else {
            Err(FactoryError::Exists)
        }
    }

    fn get_module(&mut self, name: &str) -> usize {
        let Some(&id) = self.modules.get(name) else {
            let id = *self.modules.get("__next").unwrap_or(&1);
            self.modules.insert("__next".to_owned(), id + 1);
            return id 
        };

        id
    }

    fn parts_from_exprs(&mut self, exprs: Vec<Expr>) -> Result<Vec<RecipePart>, FactoryError> {
        let mut parts = Vec::with_capacity(exprs.len());
        for expr in exprs {
            if let Some(value) = self.process_expr(expr)? {
                let recipe_part = match value {
                    Value::RecipePart(recipe_part) => recipe_part,
                    Value::Product(_, product) => RecipePart { product, amount: 1 },
                    _ => return Err(FactoryError::Gleep),
                };

                parts.push(recipe_part);
            } else {
                return Err(FactoryError::Glorp)
            }
        }

        Ok(parts)
    }

    fn usize_from_expr(&mut self, expr: Expr) -> Result<usize, FactoryError> {
        if let Some(value) = self.process_expr(expr)? {
            match value {
                Value::Int(out) => Ok(out as usize),
                _ => Err(FactoryError::Gleep)
            }
        } else {
            Err(FactoryError::Glorp)
        }
    }

    fn stream_from_expr(&mut self, expr: Expr) -> Result<Rc<RefCell<Stream>>, FactoryError> {
        fn parse_call(call: Value) -> Result<Rc<RefCell<Stream>>, FactoryError> {
            let Value::Call(lhs, rhs) = call else {
                return Err(FactoryError::Gleep);
            };

            let Value::Recipe(_, recipe) = *lhs else {
                return Err(FactoryError::Gleep)
            };

            let mut inputs = Vec::with_capacity(rhs.len());

            for value in rhs {
                match value {
                    Value::Stream(_, stream) => inputs.push(stream),
                    Value::Call(..) => {
                        inputs.push(parse_call(value)?);
                    },
                    Value::MultRecipe(call, mult) => {
                        inputs.push(parse_call(*call).inspect(|stream| stream.borrow_mut().mult = mult)?);
                    },
                    _ => {
                        println!("{value}");
                        return Err(FactoryError::Gleep)
                    },
                }
            }

            Ok(Rc::new(RefCell::new(Stream { mult: 1, recipe: recipe.clone(), inputs: inputs.into() })))
        }

        if let Some(value) = self.process_expr(expr)? {
            match value {
                Value::Call(..) => {
                    parse_call(value)
                },
                Value::MultRecipe(call, mult) => {
                    parse_call(*call).inspect(|stream| stream.borrow_mut().mult = mult)
                },
                _ => Err(FactoryError::Gleep)
            }
        } else {
            Err(FactoryError::Glorp)
        }
    }
}

fn id(name: &str, module: &str) -> String {
    format!("{module}::{name}")
}