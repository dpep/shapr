class A
  %w[a b].each { |x| define_method("get_#{x}") { x } }
  def get_a2; 1; end
  def t; get_a2; end
end
