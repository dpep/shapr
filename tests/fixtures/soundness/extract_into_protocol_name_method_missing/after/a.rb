class A
  def go(a)
    method_missing(a)
    1
  end

  def method_missing(a)
    t = a * 2 + 1
    u = t + a * 3
    v = u - t
  end
end
