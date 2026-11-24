class A
  %w[a b].each { |x| define_method("get_#{x}") { x } }
  def get_a3; 1; end
  def t; get_a3; end
end
