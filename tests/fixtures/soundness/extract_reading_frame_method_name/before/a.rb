class A
  def go(a)
    n = __method__
    puts n.to_s + a.to_s + 'x'
    1
  end
end
