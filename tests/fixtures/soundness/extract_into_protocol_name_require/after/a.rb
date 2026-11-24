class A
  def go(a)
    require(a)
    1
  end

  def require(a)
    t = a * 2 + 1
    u = t + a * 3
    v = u - t
  end
end
