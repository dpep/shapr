class A
  def go(a)
    hash(a)
    1
  end

  def hash(a)
    t = a * 2 + 1
    u = t + a * 3
    v = u - t
  end
end
