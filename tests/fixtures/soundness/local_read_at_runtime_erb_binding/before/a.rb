def f(x)
  ERB.new('<%= x %>').result(binding)
end
