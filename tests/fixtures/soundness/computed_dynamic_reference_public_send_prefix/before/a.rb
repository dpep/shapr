class A
  def go(n); public_send("get_" + n); end
  def get_a2; 1; end
  def t; get_a2; end
end
