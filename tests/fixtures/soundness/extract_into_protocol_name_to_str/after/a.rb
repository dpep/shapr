class A
  def go(a)
    to_str(a)
    1
  end

  def to_str(a)
    t = a * 2 + 1
    u = t + a * 3
    v = u - t
  end
end
