use std::{cell::RefCell, cmp::Ordering, collections::HashMap, fmt::Display, rc::Rc};

use crate::{lang::parser::{Expr, InfixOp, Literal}, rate::Rate, Buffer, Product, Recipe, RecipePart, Stream};

pub const DEFAULT_BUF_MULT: usize = 8;

#[derive(Clone, Debug)]
pub struct Factory {
    pub products: HashMap<String, Rc<RefCell<Product>>>,
    pub product_names: HashMap<Product, String>,
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
    MultRecipe(Box<Value>, usize),
    Method(Box<Method>),
    Int(isize),
    Float(f64),
    String(String),
    Bool(bool)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Method {
    pub object: Value,
    pub name: String,
}

#[derive(Clone, Debug)]
pub enum FactoryError {
    UnexpectedEof,
    TypeError,
    Exists(String),
    InvalidArguments,
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
            e => format!("{:?}", e),
        };

        write!(f, "{}", content)
    }
}

impl Factory {
    pub fn new() -> Self {
        let mut products = HashMap::new();
        let product_names = HashMap::new();
        let recipes = HashMap::new();
        let streams = HashMap::new();
        let mut modules = HashMap::new();

        products.insert("__next".to_owned(), Rc::new(RefCell::new(Product { id: 0, module: 0 })));
        modules.insert("factory".to_owned(), 0);

        Self {
            products,
            product_names,
            recipes,
            streams,
            modules,
        }
    }

    pub fn solve(&mut self, stream: Rc<RefCell<Stream>>) {
        let efficiency = stream.borrow().efficiency();
        let mut changes: Vec<(Rc<RefCell<Stream>>, usize)> = Vec::with_capacity(4);

        if efficiency < 1.0 {
            // balance and solve each input
            for input in &stream.borrow().inputs.inner {
                for ingredient in &stream.borrow().recipe.borrow().inputs {
                    let product = &*ingredient.product.borrow();
                    if let Some(optimal) = stream.borrow().recipe.borrow().optimal_inflow_of(product) {
                        let optimal = optimal * stream.borrow().mult;

                        if let Some(rate) = input.1.borrow().recipe.borrow().optimal_outflow_of(product) {
                            let rate = rate * input.1.borrow().mult;

                            if rate < optimal {
                                let efficiency = rate / optimal;
                                let mult = 1.0 / efficiency;
                                let new_mult = input.1.borrow().mult as f64 * mult;
                                changes.push((input.1.clone(), (new_mult - f64::EPSILON).ceil() as usize));
                            }
                        }
                        
                        if let Some(rate) = input.1.borrow().rate_of(product) {
                            if rate < optimal {
                                self.solve(input.1.clone());
                            }
                        }
                    }
                }
            }
        }

        for (stream, mult) in changes {
            let old_mult = stream.borrow().mult;
            let mult_mult = mult / old_mult;

            for (_, buf) in stream.borrow_mut().buffers.iter_mut() {
                buf.max *= mult_mult;
            }

            stream.borrow_mut().mult = mult;

            self.solve(stream.clone());
        }
    }

    pub fn add_mod(&mut self, mut ast: Vec<Expr>) -> Result<(), FactoryError> {
        ast.sort_unstable_by(|lhs, rhs| {
            match (lhs, rhs) {
                (Expr::Product { .. }, Expr::Product { .. }) => Ordering::Equal,
                (Expr::Product { .. }, _) => Ordering::Less,
                (_, Expr::Product { .. }) => Ordering::Greater,
                (_, _) => Ordering::Equal,
            }
        });
        
        for expr in ast {
            self.process_expr(expr, "base")?;
        }

        Ok(())
    }

    pub fn add_factory(&mut self, ast: Vec<Expr>) -> Result<(), FactoryError> {
        for expr in ast {
            self.process_user_expr(expr)?;
        }

        Ok(())
    }

    pub fn process_user_expr(&mut self, expr: Expr) -> Result<(), FactoryError> {
        match expr {
            Expr::Product { .. }
            | Expr::Recipe { .. } => {},
            _ => { self.process_expr(expr, "factory")?; }
        }

        Ok(())
    }

    fn process_expr(&mut self, expr: Expr, module: &str) -> Result<Option<Value>, FactoryError> {
        match expr {
            Expr::Product { name } => {
                self.register_product(&name, module)?;
                Ok(None)
            },
            Expr::Recipe { name, inputs, outputs, period } => {
                self.register_recipe(&name, inputs, outputs, *period, module)?;
                Ok(None)
            },
            Expr::Assign { name, rhs } => {
                self.register_stream(&name, *rhs, module)?;
                Ok(None)
            }
            Expr::Ident(ident) => {
                if let Some(stream) = self.streams.get(&ident) {
                    Ok(Some(Value::Stream(ident, stream.clone())))
                } else if let Some(recipe) = self.recipes.get(&ident) {
                    Ok(Some(Value::Recipe(ident, recipe.clone())))
                } else if let Some(product) = self.products.get(&ident) {
                    Ok(Some(Value::Product(ident, product.clone())))
                } else {
                    panic!("Undefined identifier: {ident}");
                }
            },
            Expr::Call { lhs, args } => {
                let lhs = self.process_expr(*lhs, module)?.unwrap();
                let mut args_out = Vec::with_capacity(args.len());

                for expr in args {
                    if let Some(value) = self.process_expr(expr, module)? {
                        args_out.push(value);
                    } else {
                        return Err(FactoryError::UnexpectedEof)
                    }
                }

                match lhs {
                    Value::Method(method) => {
                        self.call(*method, args_out)
                    },
                    Value::Recipe(..) => {
                        Ok(Some(Value::Call(Box::new(lhs), args_out)))
                    },
                    _ => unimplemented!()
                }
            }
            Expr::InfixOp { lhs, op, rhs } => {
                // unwrapping is fine here cause only expressions that return Some from this method should be put in an InfixOp
                let lhs = self.process_expr(*lhs, module)?.unwrap();
                let rhs = self.process_expr(*rhs, module)?.unwrap();

                Ok(Some(self.process_op(lhs, op, rhs)))
            },
            Expr::Literal(literal) => {
                Ok(Some(match literal {
                    Literal::Int(e) => Value::Int(e),
                    Literal::Float(e)  => Value::Float(e),
                    Literal::String(e) => Value::String(e),
                    Literal::Bool(e) => Value::Bool(e),
                }))
            },
            Expr::Access { lhs, rhs } => {
                let lhs = self.process_expr(*lhs, module)?.unwrap();

                Ok(Some(lhs.access(&rhs)))
            },
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
            },
            (Value::Int(lhs), InfixOp::Mul, Value::Int(rhs)) => Value::Int(lhs * rhs),
            (lhs, op, rhs) => panic!("Invalid operation: `{lhs:?} {op:?} {rhs:?}`"),
        }
    }

    fn register_product(&mut self, name: &str, module: &str) -> Result<(), FactoryError> {
        if self.products.get(name).is_none() {
            let module_id = self.get_module(module);
            let product_id = self.products.get("__next").map(|i| i.borrow().id).unwrap_or(0);
            let product = Product { id: product_id, module: module_id };

            self.products.insert("__next".to_owned(), Rc::new(RefCell::new(Product { id: product_id + 1, module: 0 })));
            self.products.insert(name.to_owned(), Rc::new(RefCell::new(product)));
            self.product_names.insert(product, name.to_owned());

            Ok(())
        } else {
            Err(FactoryError::Exists(name.to_owned()))
        }
    }

    fn register_recipe(&mut self, name: &str, inputs: Vec<Expr>, outputs: Vec<Expr>, period: Expr, module: &str) -> Result<(), FactoryError> {
        if self.recipes.get(name).is_none() {
            let inputs = self.parts_from_exprs(inputs, module)?;
            let outputs = self.parts_from_exprs(outputs, module)?;
            let period = self.usize_from_expr(period, module)?;
            let rate = Rate { amount: 1, ticks: period as f64 };
            let recipe = Recipe {
                rate,
                inputs,
                outputs,
            };

            self.recipes.insert(name.to_owned(), Rc::new(RefCell::new(recipe)));

            Ok(())
        } else {
            Err(FactoryError::Exists(name.to_owned()))
        }
    }

    fn register_stream(&mut self, name: &str, expr: Expr, module: &str) -> Result<(), FactoryError> {
        if self.streams.get(name).is_none() {
            let stream = self.stream_from_expr(expr, module)?;

            self.streams.insert(name.to_owned(), stream);

            Ok(())
        } else {
            Err(FactoryError::Exists(name.to_owned()))
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

    fn parts_from_exprs(&mut self, exprs: Vec<Expr>, module: &str) -> Result<Vec<RecipePart>, FactoryError> {
        let mut parts = Vec::with_capacity(exprs.len());
        for expr in exprs {
            if let Some(value) = self.process_expr(expr, module)? {
                let recipe_part = match value {
                    Value::RecipePart(recipe_part) => recipe_part,
                    Value::Product(_, product) => RecipePart { product, amount: 1 },
                    _ => return Err(FactoryError::TypeError),
                };

                parts.push(recipe_part);
            } else {
                return Err(FactoryError::UnexpectedEof)
            }
        }

        Ok(parts)
    }

    fn usize_from_expr(&mut self, expr: Expr, module: &str) -> Result<usize, FactoryError> {
        if let Some(value) = self.process_expr(expr, module)? {
            match value {
                Value::Int(out) => Ok(out as usize),
                _ => Err(FactoryError::TypeError)
            }
        } else {
            Err(FactoryError::UnexpectedEof)
        }
    }

    fn stream_from_expr(&mut self, expr: Expr, module: &str) -> Result<Rc<RefCell<Stream>>, FactoryError> {
        if let Some(value) = self.process_expr(expr, module)? {
            match value {
                Value::Call(..) => {
                    self.parse_call(value)
                },
                Value::MultRecipe(call, mult) => {
                    self.parse_call(*call).inspect(|stream| stream.borrow_mut().mult = mult)
                },
                _ => Err(FactoryError::TypeError)
            }
        } else {
            Err(FactoryError::UnexpectedEof)
        }
    }

    fn parse_call(&mut self, call: Value) -> Result<Rc<RefCell<Stream>>, FactoryError> {
        let Value::Call(lhs, rhs) = call else {
            return Err(FactoryError::TypeError);
        };

        let Value::Recipe(_, recipe) = *lhs else {
            return Err(FactoryError::TypeError)
        };

        let mut inputs = Vec::with_capacity(rhs.len());

        if rhs.len() != recipe.borrow().inputs.len() {
            return Err(FactoryError::InvalidArguments);
        }

        for (idx, value) in rhs.into_iter().enumerate() {
            let product = recipe.borrow().inputs[idx].product.clone();

            match value {
                Value::Stream(_, stream) => inputs.push((product, stream)),
                Value::Call(..) => {
                    inputs.push((product, self.parse_call(value)?));
                },
                Value::MultRecipe(call, mult) => {
                    inputs.push((product, self.parse_call(*call).inspect(|stream| stream.borrow_mut().mult = mult)?));
                },
                _ => {
                    println!("{value}");
                    return Err(FactoryError::TypeError)
                },
            }
        }

        let mut buffer = HashMap::new();

        for output in &recipe.borrow().outputs {
            let product = output.product.borrow().clone();
            buffer.insert(product, Buffer { current: 0, max: output.amount * DEFAULT_BUF_MULT});
        }

        let ticks = recipe.borrow().rate.ticks as usize;
        Ok(Rc::new(RefCell::new(Stream { mult: 1, recipe: recipe.clone(), inputs: inputs.into(), buffers: buffer, next: None, ticks })))
    }

    pub fn call(&mut self, method: Method, args: Vec<Value>) -> Result<Option<Value>, FactoryError> {
        match (method.object, method.name) {
            (Value::Stream(stream_name, stream), name) => {
                match name.as_ref() {
                    "buffer" => match args.as_slice() {
                        &[Value::Product(_, ref product), Value::Int(buffer)] => {
                            stream.borrow_mut().buffers.get_mut(&product.borrow()).unwrap().max = buffer as usize;
                            Ok(None)
                        },
                        _ => Err(FactoryError::InvalidArguments)
                    },
                    "solve" => match args.as_slice() {
                        &[] => {
                            self.solve(stream.clone());
                            Ok(None)
                        }
                        _ => Err(FactoryError::InvalidArguments)
                    },
                    "log" => {
                        let outputs = if args.len() == 0 {
                            stream.borrow().recipe.borrow().outputs.clone()
                        } else {
                            let mut out = Vec::new();

                            for product in args {
                                if let Value::Product(_, product) = product {
                                    if let Some(recipe_part) = stream.borrow().recipe.borrow().outputs.iter().find(|e| e.product == product) {
                                        out.push(recipe_part.clone());
                                    }
                                }

                            }

                            out
                        };

                        println!("----- {stream_name} x{} -----", stream.borrow().mult);
                        for output in outputs {
                            // if the product isnt in the stream something went wrong so a panic is actually desired
                            let rate = stream.borrow().rate_of(&*output.product.borrow()).unwrap();
                            let name = self.product_names.get(&*output.product.borrow()).unwrap();
                            println!("  {} @ {}", name, rate);
                        }

                        Ok(None)
                    }
                    _ => unimplemented!()
                }
            },
            _ => unimplemented!()
        }
    }

    pub fn tick(&mut self, ticks: usize) {
        for stream in self.streams.values() {
            let mut ticks = ticks;
            let mut cycles = 0;
            let reset = stream.borrow().recipe.borrow().rate.ticks as usize;
            let mut next = stream.borrow().next.unwrap_or(reset);

            while ticks > 0 {
                if ticks > reset {
                    cycles += ticks / reset;
                    ticks = ticks % reset;
                } else if ticks >= next {
                    ticks -= next;
                    cycles += 1;
                } else {
                    next -= ticks;
                    ticks = 0;
                }
            }

            stream.borrow_mut().next = Some(next);

            let mut produced: Vec<RecipePart> = stream.borrow().recipe.borrow().outputs.clone().iter().map(|output| RecipePart { product: output.product.clone(), amount: 0 }).collect();

            for _ in 0..cycles {
                let inputs = stream.borrow().inputs.clone();

                for (product, input) in inputs.inner {
                    if let Some(buffer) = input.borrow_mut().buffers.get_mut(&*product.borrow()) {
                        let mut own_buffer = stream.borrow().buffers.get(&*product.borrow()).cloned().unwrap_or_else(|| {
                            let max = stream.borrow().recipe.borrow().required_of(&*product.borrow()).unwrap() * DEFAULT_BUF_MULT * stream.borrow().mult;

                            Buffer { current: 0, max }
                        });

                        own_buffer.fill_from(buffer);
                        stream.borrow_mut().buffers.insert(*&*product.borrow(), own_buffer);
                    }
                }

                if stream.borrow_mut().try_produce() {
                    for (idx, output) in stream.borrow().recipe.borrow().outputs.iter().enumerate() {
                        if produced[idx].product == output.product {
                            produced[idx].amount += output.amount;
                        } else {
                            produced.iter_mut().find(|produced| produced.product == output.product).unwrap().amount += output.amount;
                        }
                    }
                } else {
                    break;
                }
            }
            
            for output in produced {
                if output.amount > 0 {
                    println!("Produced {} x{} ({})", self.product_names.get(&*output.product.borrow()).unwrap(), output.amount * stream.borrow().mult, stream.borrow().buffers.get(&*output.product.borrow()).unwrap());
                }
            }
        }

        println!();
    }
}

impl Value {
    pub fn access(&self, rhs: &str) -> Value {
        match self {
            Self::Stream(..) => {
                match rhs {
                    "buffer"
                    | "solve"
                    | "log" => Value::Method(Box::new(Method { object: self.clone(), name: rhs.to_owned() })),
                    _ => unimplemented!(),
                }
            },
            _ => unimplemented!(),
        }
    }
}